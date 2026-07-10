//! The CLI's runtime host: platform wiring shared by `run` and `dev`.
//!
//! A [`RuntimeHost`] owns everything that outlives a single VM — the
//! tokio runtime, the shared network handle table, the function store
//! (Execute), and the process runtime — and knows how to build a fully
//! wired VM for any process. Both `ambient run` and `ambient dev` are
//! thin drivers over [`RuntimeHost::deploy`]: `run` deploys once and
//! waits for the process tree to finish; `dev` deploys again on every
//! code change.
//!
//! Wiring is native registration, uuid-keyed: stubs first (every platform
//! extern answers with a catchable "not wired" exception), then the real
//! implementation sets, which overwrite the stubs they cover. Process
//! natives are installed per process VM by the [`ProcessRuntime`] itself.

use std::sync::Arc;

use anyhow::{Result, bail};

use ambient_engine::compiler::CompiledModule;
use ambient_engine::natives::NativeRegistry;
use ambient_engine::store::Store;
use ambient_engine::vm::Vm;
use ambient_platform::process::{
    DeployOutcome, EventSink, ProcessRuntime, ProcessRuntimeConfig, functions_from_module,
};
use ambient_platform::{ExecuteConfig, NetworkState, StdioConfig, StdioSink};

/// Long-lived platform state plus the process runtime.
pub struct RuntimeHost {
    /// Owns the reactor threads driving all network IO. Kept alive for
    /// the host's lifetime; unused otherwise.
    _tokio: tokio::runtime::Runtime,
    store: Arc<std::sync::Mutex<Store>>,
    runtime: Arc<ProcessRuntime>,
}

impl RuntimeHost {
    /// Build the host: share one network table and one store across every
    /// future VM, and start an (empty) process runtime.
    ///
    /// `program_args` become `core::system::Env::args!()` for every VM
    /// this host builds (`ambient run` composes the program path plus the
    /// trailing args; `ambient dev` passes an empty vec — program args
    /// have no coherent meaning across reconciliation re-deploys).
    pub fn new(events: EventSink, program_args: Vec<String>) -> Result<Self> {
        let tokio = tokio::runtime::Runtime::new()
            .map_err(|e| anyhow::anyhow!("failed to create async runtime: {e}"))?;

        let network_state = Arc::new(NetworkState::new(tokio.handle().clone()));
        let store = Arc::new(std::sync::Mutex::new(Store::new()));
        let argv = Arc::new(program_args);

        // Log's default implementations perform Stdio, so both stream to
        // the same stdout for every VM this host builds.
        let sink = StdioSink::default();

        // Host policy for executed-by-hash code (Execute ability): shipped
        // code may print (Stdio, and Log through it) but gets no
        // FileSystem, Network, Time, Random, or recursive Execute — their
        // performs run the default implementations, whose extern calls
        // land on the stubs and raise catchable "not wired" exceptions.
        let exec_grants = {
            let sink = sink.clone();
            Arc::new(move |exec_vm: &mut Vm| {
                exec_vm.register_natives(&ambient_platform::stub_natives());
                exec_vm.register_natives(&ambient_platform::stdio_natives(
                    sink.clone(),
                    StdioConfig::default(),
                ));
            })
        };

        // One registry set, built once; each VM registration is
        // uuid-keyed, so the real sets overwrite the stubs they cover.
        let mut sets: Vec<NativeRegistry> = vec![
            ambient_platform::stub_natives(),
            ambient_platform::stdio_natives(sink, StdioConfig::default()),
            ambient_platform::time_natives(),
            ambient_platform::random_natives(),
            ambient_platform::fs_natives(),
            ambient_platform::env_natives(Arc::clone(&argv)),
            ambient_platform::network_natives(Arc::clone(&network_state)),
        ];
        sets.push(ambient_platform::execute_natives(ExecuteConfig {
            store: Arc::clone(&store),
            grants: Some(exec_grants as _),
        }));
        let sets = Arc::new(sets);

        let factory = {
            let sets = Arc::clone(&sets);
            Arc::new(move || {
                let mut vm = Vm::new();
                for set in sets.iter() {
                    vm.register_natives(set);
                }
                vm
            })
        };

        let runtime = ProcessRuntime::new(ProcessRuntimeConfig {
            vm_factory: factory,
            events,
        });

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
        // an exact key; else any *module-path* key ending in `::{entry}` —
        // uuid-rooted dispatch symbols (`<uuid>::run`, e.g. Execute's own
        // method impl) are never entry points and are skipped; else, for
        // the default entry, the structurally-captured entry point.
        let suffix = format!("::{entry}");
        let entry_hash = compiled
            .function_names
            .get(entry)
            .or_else(|| {
                compiled
                    .function_names
                    .iter()
                    .find(|(name, _)| name.ends_with(&suffix) && !is_dispatch_symbol(name))
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

/// Whether a store name is a content dispatch symbol (`<uuid>::method` for
/// ability default implementations and nominal-type methods) rather than a
/// module-qualified function name.
fn is_dispatch_symbol(name: &str) -> bool {
    name.split("::")
        .next()
        .is_some_and(|head| uuid::Uuid::parse_str(head).is_ok())
}
