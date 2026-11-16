//! Definitions, imports, and the module they make.

use super::super::lex::Token;
use super::{Parsed, Parser};
use crate::builder::{self as b, ModuleBuilder};
use crate::surface::{
    Ability, Definition, DefinitionKind, Extern, ExternKind, Function, GENERATOR, KeyDefinition,
    Param, Path, Target, Type, TypeDefinition,
};
use crate::syntax::{Placed, SyntaxError, place};

impl Parser {
    /// The whole file.
    pub(in crate::syntax) fn module(mut self) -> Parsed<crate::syntax::Source> {
        let mut module = b::module();
        while *self.peek() != Token::End {
            module = self.item(module)?;
        }
        // What a builder refuses is about the module as a whole.
        let module = module.build().map_err(|error| SyntaxError {
            message: error.to_string(),
            at: crate::syntax::Position { line: 1, column: 1 },
        })?;
        Ok(crate::syntax::Source {
            module,
            definitions: self.defined,
        })
    }

    /// One import or definition, added to `module`.
    fn item(&mut self, module: ModuleBuilder) -> Parsed<ModuleBuilder> {
        let doc = match self.peek().clone() {
            Token::Doc(doc) => {
                self.advance();
                Some(doc)
            }
            _ => None,
        };
        let public = self.eat_word("pub");
        if self.eat_word("import") {
            return self.import(module, public);
        }
        if public && self.eat_word("module") {
            let path = self.path()?;
            self.expect(";")?;
            return Ok(module.expose(&path.to_string()));
        }
        let at = self.position();
        self.defined.push(Placed {
            at,
            nodes: Vec::new(),
        });
        let (name, mut kind) = self.definition()?;
        // The marks come out here, before anything else sees the tree.
        let body = match &mut kind {
            DefinitionKind::Function(function) => Some(&mut function.body),
            DefinitionKind::Derived(call)
            | DefinitionKind::Generated(call)
            | DefinitionKind::Module(call) => Some(call),
            _ => None,
        };
        if let (Some(body), Some(placed)) = (body, self.defined.last_mut()) {
            place::lift(body, &self.positions, at, &mut placed.nodes);
        }
        Ok(module.define(Definition {
            name,
            public,
            doc,
            kind,
        }))
    }

    /// `import a.b;`, `import a.b as c;`, or `import a.{b, c as d};`.
    fn import(&mut self, mut module: ModuleBuilder, public: bool) -> Parsed<ModuleBuilder> {
        let mut prefix = vec![self.name()?];
        let mut taken: Vec<(Path, Option<String>)> = Vec::new();
        loop {
            if !self.eat(".") {
                let alias = self.alias()?;
                taken.push((Path(prefix), alias));
                break;
            }
            if self.eat("{") {
                taken = self.list("}", |parser| {
                    let mut path = prefix.clone();
                    path.push(parser.name()?);
                    Ok((Path(path), parser.alias()?))
                })?;
                break;
            }
            prefix.push(self.name()?);
        }
        self.expect(";")?;
        for (path, alias) in taken {
            let path = path.to_string();
            module = match (alias, public) {
                (Some(_), true) => return self.fail("a re-export keeps the name it has"),
                (Some(alias), false) => module.import_as(&path, &alias),
                (None, true) => module.export(&path),
                (None, false) => module.import(&path),
            };
        }
        Ok(module)
    }

    fn alias(&mut self) -> Parsed<Option<String>> {
        if self.eat_word("as") {
            Ok(Some(self.name()?))
        } else {
            Ok(None)
        }
    }

    fn definition(&mut self) -> Parsed<(String, DefinitionKind)> {
        // The call a generated module is written as.
        if self.at_word("comptime") {
            let call = self.expr()?;
            self.expect(";")?;
            return Ok((GENERATOR.to_owned(), DefinitionKind::Module(call)));
        }
        if self.eat_word("fn") {
            return self.function();
        }
        if self.eat_word("type") {
            return self.type_definition();
        }
        if self.eat_word("ability") {
            let name = self.name()?;
            let type_params = self.type_params()?;
            let params = self.params()?;
            let ret = self.returns()?;
            self.expect(";")?;
            let ability = Ability {
                type_params,
                params,
                ret,
            };
            return Ok((name, DefinitionKind::Ability(ability)));
        }
        if self.eat_word("key") {
            return self.key();
        }
        if self.eat_word("tag") {
            let name = self.name()?;
            self.expect(";")?;
            return Ok((name, DefinitionKind::Tag));
        }
        if self.eat_word("brand") {
            let name = self.name()?;
            self.expect(";")?;
            return Ok((name, DefinitionKind::Brand));
        }
        if self.eat_word("extern") {
            return self.native();
        }
        if self.eat_word("target") {
            return self.target();
        }
        self.unexpected("a definition")
    }

    /// `(name: Type, ...)`
    fn params(&mut self) -> Parsed<Vec<Param>> {
        self.expect("(")?;
        self.list(")", |parser| {
            let name = parser.name()?;
            parser.expect(":")?;
            Ok(Param {
                name,
                ty: parser.ty()?,
            })
        })
    }

    /// `: Type`, or `null` where nothing is written.
    fn returns(&mut self) -> Parsed<Type> {
        if self.eat(":") {
            self.ty()
        } else {
            Ok(Type::Null)
        }
    }

    fn function(&mut self) -> Parsed<(String, DefinitionKind)> {
        let name = self.name()?;
        // A function that is whatever a comptime call gives.
        if self.eat("=") {
            if !self.at_word("comptime") {
                return self.fail("expected a comptime call after `=`");
            }
            let call = self.expr()?;
            self.expect(";")?;
            return Ok((name, DefinitionKind::Generated(call)));
        }
        let type_params = self.type_params()?;
        let params = self.params()?;
        let ret = self.returns()?;
        let row = self.row()?;
        let arrow = self.at("=>");
        let body = self.scoped(|parser| {
            parser
                .locals
                .extend(params.iter().map(|param| param.name.clone()));
            parser.body()
        })?;
        if arrow {
            self.expect(";")?;
        }
        let function = Function {
            type_params,
            params,
            ret,
            row,
            body,
        };
        Ok((name, DefinitionKind::Function(function)))
    }

    fn type_definition(&mut self) -> Parsed<(String, DefinitionKind)> {
        let name = self.name()?;
        let params = if self.eat("<") {
            self.list(">", Self::name)?
        } else {
            Vec::new()
        };
        self.expect("=")?;
        if params.is_empty() && self.at_word("comptime") {
            let call = self.expr()?;
            self.expect(";")?;
            return Ok((name, DefinitionKind::Derived(call)));
        }
        let named = |param: &String| Type::Named(Path(vec![param.clone()]), Vec::new());
        self.own = Some(Type::Named(
            Path(vec![name.clone()]),
            params.iter().map(named).collect(),
        ));
        let ty = self.ty();
        self.own = None;
        let ty = ty?;
        self.expect(";")?;
        Ok((name, DefinitionKind::Type(TypeDefinition { params, ty })))
    }

    /// `key name: Type;`, or `key name(params): Type;` for a key that
    /// holds a function.
    fn key(&mut self) -> Parsed<(String, DefinitionKind)> {
        let name = self.name()?;
        let ty = if self.eat(":") {
            self.ty()?
        } else {
            let params = self.params()?;
            Type::Function {
                params: params.into_iter().map(|param| param.ty).collect(),
                ret: Box::new(self.returns()?),
                row: self.row()?,
            }
        };
        self.expect(";")?;
        Ok((name, DefinitionKind::Key(KeyDefinition { ty })))
    }

    /// `extern fn name(params): Type;` or
    /// `extern handler name for ability with row;`, which may be
    /// `extern suspending handler`.
    fn native(&mut self) -> Parsed<(String, DefinitionKind)> {
        let suspends = self.eat_word("suspending");
        let (name, kind) = if !suspends && self.eat_word("fn") {
            let name = self.name()?;
            let params = self.params()?;
            let ret = self.returns()?;
            (name, ExternKind::Function { params, ret })
        } else {
            self.expect_word("handler")?;
            let name = self.name()?;
            self.expect_word("for")?;
            let ability = self.path()?;
            let row = self.row()?;
            (name, ExternKind::Handler { ability, row })
        };
        self.expect(";")?;
        Ok((name, DefinitionKind::Extern(Extern { kind, suspends })))
    }

    /// `target name extends a, b { ability: handler, ... }`
    fn target(&mut self) -> Parsed<(String, DefinitionKind)> {
        let name = self.name()?;
        let mut extends = Vec::new();
        if self.eat_word("extends") {
            extends.push(self.path()?);
            while self.eat(",") {
                extends.push(self.path()?);
            }
        }
        self.expect("{")?;
        let bindings = self.list("}", |parser| {
            let ability = parser.path()?;
            parser.expect(":")?;
            Ok((ability, parser.path()?))
        })?;
        Ok((name, DefinitionKind::Target(Target { extends, bindings })))
    }
}
