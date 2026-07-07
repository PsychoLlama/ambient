//! The CLI's runtime host: platform wiring shared by `run` and `dev`.
//!
//! A [`RuntimeHost`] owns everything that outlives a single VM — the
//! tokio runtime, the shared network handle table, the function store
//! (Execute), and the process runtime — and knows how to build a fully
//! wired VM for any process. Both `ambient run` and `ambient dev` are
//! thin drivers over [`RuntimeHost::deploy`]: `run` deploys once and
//! waits for the process tree to finish; `dev` deploys again on every
//! code change.

use std::sync::Arc;

use anyhow::{Context, Result, bail};

use ambient_engine::compiler::CompiledModule;
use ambient_engine::store::Store;
use ambient_engine::vm::Vm;
use ambient_platform::process::{
    DeployOutcome, EventSink, ProcessRuntime, ProcessRuntimeConfig, functions_from_module,
};
use ambient_platform::{
    ExecuteConfig, LogConfig, NetworkState, StdioConfig, StdioSink, register_env, register_execute,
    register_fs, register_log, register_network_shared, register_random, register_stdio,
    register_time,
};

use super::{platform_prelude, prelude_interface};

/// Long-lived platform state plus the process runtime.
pub struct RuntimeHost {
    /// Owns the reactor threads driving all network IO. Kept alive for
    /// the host's lifetime; unused otherwise.
    _tokio: tokio::runtime::Runtime,
    store: Arc<std::sync::Mutex<Store>>,
    runtime: Arc<ProcessRuntime>,
}

impl RuntimeHost {
    /// Build the host: resolve the platform prelude, share one network
    /// table and one store across every future VM, and start an (empty)
    /// process runtime.
    ///
    /// `program_args` become `core::system::Env::args!()` for every VM
    /// this host builds (`ambient run` composes the program path plus the
    /// trailing args; `ambient dev` passes an empty vec — program args
    /// have no coherent meaning across reconciliation re-deploys).
    pub fn new(events: EventSink, program_args: Vec<String>) -> Result<Self> {
        let tokio = tokio::runtime::Runtime::new().context("failed to create async runtime")?;
        let prelude = platform_prelude()?;

        let stdio = prelude_interface(&prelude, "Stdio")?;
        let time = prelude_interface(&prelude, "Time")?;
        let random = prelude_interface(&prelude, "Random")?;
        let log = prelude_interface(&prelude, "Log")?;
        let fs = prelude_interface(&prelude, "FileSystem")?;
        let network = prelude_interface(&prelude, "Network")?;
        let execute = prelude_interface(&prelude, "Execute")?;
        let process = prelude_interface(&prelude, "Process")?;
        let env = prelude_interface(&prelude, "Env")?;

        let network_state = Arc::new(NetworkState::new(tokio.handle().clone()));
        let store = Arc::new(std::sync::Mutex::new(Store::new()));
        let argv = Arc::new(program_args);

        // Log shares Stdio's output sink, so both stream to the same
        // stdout for every VM this host builds.
        let sink = StdioSink::default();

        // Every process VM gets the full platform set. Executed-by-hash
        // code (Execute ability) stays restricted to Stdio + Log, as
        // before.
        let exec_grants = {
            let stdio = stdio.clone();
            let log = log.clone();
            let sink = sink.clone();
            Arc::new(move |exec_vm: &mut Vm| {
                register_stdio(exec_vm, &stdio, sink.clone(), StdioConfig::default());
                register_log(exec_vm, &log, LogConfig::default(), sink.clone());
            })
        };

        let factory_store = Arc::clone(&store);
        let factory = {
            let network_state = Arc::clone(&network_state);
            let argv = Arc::clone(&argv);
            Arc::new(move || {
                let mut vm = Vm::new();
                register_stdio(&mut vm, &stdio, sink.clone(), StdioConfig::default());
                register_time(&mut vm, &time);
                register_random(&mut vm, &random);
                register_log(&mut vm, &log, LogConfig::default(), sink.clone());
                register_fs(&mut vm, &fs);
                register_env(&mut vm, &env, Arc::clone(&argv));
                register_network_shared(&mut vm, &network, Arc::clone(&network_state));
                register_execute(
                    &mut vm,
                    &execute,
                    ExecuteConfig {
                        store: Arc::clone(&factory_store),
                        grants: Some(Arc::clone(&exec_grants) as _),
                    },
                );
                vm
            })
        };

        let runtime = ProcessRuntime::new(
            ProcessRuntimeConfig {
                vm_factory: factory,
                interface: process,
                events,
            },
            Arc::new(std::collections::HashMap::new()),
        );

        Ok(Self {
            _tokio: tokio,
            store,
            runtime,
        })
    }

    /// Deploy a build: install it as the current code generation and run
    /// `entry` as a reconciliation pass over the live process tree.
    pub fn deploy(&self, compiled: &CompiledModule, entry: &str) -> Result<DeployOutcome> {
        // Function names in the merged build carry their full canonical
        // qualifier (`workspace::<pkg>::main::run`). Resolve `entry` in order:
        // an exact key; else any key ending in `::{entry}`, which lets a
        // friendly bare form (`run`) or a partially-qualified one
        // (`repl::__repl_entry_1`) name a fully-qualified function; else, for
        // the default entry, the structurally-captured entry point.
        let suffix = format!("::{entry}");
        let entry_hash = compiled
            .function_names
            .get(entry)
            .or_else(|| {
                compiled
                    .function_names
                    .iter()
                    .find(|(name, _)| name.ends_with(&suffix))
                    .map(|(_, hash)| hash)
            })
            .or_else(|| {
                (entry == "run")
                    .then_some(compiled.entry_point.as_ref())
                    .flatten()
            })
            .ok_or_else(|| anyhow::anyhow!("entry function `{entry}` not found"))?;

        // Execute serves functions from the store; register every
        // generation so shipped code keeps resolving.
        match self.store.lock() {
            Ok(mut store) => store.add_module(compiled),
            Err(_) => bail!("function store lock poisoned"),
        }

        let functions = functions_from_module(compiled);
        self.runtime
            .deploy(&functions, entry_hash)
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// The process runtime (for waiting / inspection).
    pub fn runtime(&self) -> &Arc<ProcessRuntime> {
        &self.runtime
    }
}
