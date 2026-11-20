//! Types, rows, and type parameters.

use super::super::lex::Token;
use super::{Parsed, Parser};
use crate::builder::ty;
use crate::literal::Literal;
use crate::surface::{Key, Members, Path, Type, TypeParam};

/// What a record member written with parameters is: its parameter
/// types, what it gives, its row, and what it needs of its receiver if
/// its first parameter is `self`.
struct Signature {
    type_params: Vec<TypeParam>,
    receiver: Receiver,
    params: Members,
    ret: Type,
    row: Vec<Path>,
}

/// The `self` a signature begins with.
enum Receiver {
    /// None: a function.
    Absent,
    /// `self`, with nothing said of what it needs.
    Unstated,
    /// `self: Type`
    Needs(Type),
}

impl Parser {
    /// A type: one or more members separated by `|`.
    pub(super) fn ty(&mut self) -> Parsed<Type> {
        let first = self.ty_member()?;
        if !self.at("|") {
            return Ok(first);
        }
        let mut members = vec![first];
        while self.eat("|") {
            members.push(self.ty_member()?);
        }
        Ok(Type::Union(members))
    }

    /// What a function gives: a type a row may follow. A function type
    /// there is written in parentheses, since the row after one written
    /// bare could be either's.
    pub(super) fn given(&mut self) -> Parsed<Type> {
        self.typed(true, Self::ty)
    }

    /// Reads with `read`, saying meanwhile whether what is read is
    /// what a function gives with nothing around it.
    fn typed<T>(&mut self, gives: bool, read: impl FnOnce(&mut Self) -> Parsed<T>) -> Parsed<T> {
        let outer = std::mem::replace(&mut self.gives, gives);
        let read = read(self);
        self.gives = outer;
        read
    }

    /// A member of a union: a type, and the brands on it if any.
    fn ty_member(&mut self) -> Parsed<Type> {
        let ty = self.ty_single()?;
        let mut brands = Vec::new();
        while self.eat("&") {
            brands.push(self.ty_single()?);
        }
        Ok(if brands.is_empty() {
            ty
        } else {
            Type::Branded(Box::new(ty), brands)
        })
    }

    fn ty_single(&mut self) -> Parsed<Type> {
        if self.gives && !self.at_word("fn") {
            // Whatever else it is closes itself: nothing inside it is
            // followed by the row.
            return self.typed(false, Self::ty_single);
        }
        match self.peek().clone() {
            Token::Int(value) => {
                self.advance();
                Ok(Type::Literal(Literal::Int(value)))
            }
            Token::Float(value) => {
                self.advance();
                Ok(Type::Literal(Literal::float(value)))
            }
            Token::Text(text) => {
                self.advance();
                Ok(Type::Literal(Literal::String(text)))
            }
            Token::Bytes(bytes) => {
                self.advance();
                Ok(Type::Literal(Literal::Bytes(bytes)))
            }
            Token::Mark("-") => {
                self.advance();
                match self.advance() {
                    Token::Int(value) => Ok(Type::Literal(Literal::Int(-value))),
                    Token::Float(value) => Ok(Type::Literal(Literal::float(-value))),
                    _ => self.fail("`-` in a type is followed by a number"),
                }
            }
            Token::Mark("(") => {
                let at = self.position();
                self.advance();
                let members = self.list(")", |parser| Ok((parser.label(), parser.ty()?)))?;
                let (names, types): (Vec<_>, Vec<_>) = members.into_iter().unzip();
                let names: Option<Vec<String>> = names.into_iter().collect();
                match (self.grouped(at, types)?, names) {
                    (Ok(one), _) => Ok(one),
                    (Err(types), Some(names)) => Ok(Type::Tuple(Members::named(names, types))),
                    (Err(_), None) => {
                        self.fail_at(at, "a member of a tuple type is named, as `count: int`")
                    }
                }
            }
            Token::Mark("{") => self.record_type(),
            Token::Word(word) => self.named_type(&word),
            _ => self.unexpected("a type"),
        }
    }

    /// A name and the colon after it, if they are next: what a member
    /// of a tuple type and a parameter in a type are named, for a
    /// reader. The name is no part of the type.
    fn label(&mut self) -> Option<String> {
        if !(self.at_name() && matches!(self.peek_at(1), Token::Mark(":"))) {
            return None;
        }
        let name = self.name().ok()?;
        self.advance();
        Some(name)
    }

    /// The token before the one just taken.
    pub(super) fn peek_back(&self) -> &Token {
        &self.tokens[self.index.saturating_sub(2)]
    }

    fn named_type(&mut self, word: &str) -> Parsed<Type> {
        let simple = match word {
            "never" => Some(Type::Never),
            "unknown" => Some(Type::Unknown),
            "null" => Some(Type::Null),
            "bool" => Some(Type::Bool),
            "int" => Some(Type::Int),
            "float" => Some(Type::Float),
            "string" => Some(Type::String),
            "bytes" => Some(Type::Bytes),
            "Tag" => Some(Type::Tag),
            "true" => Some(Type::Literal(Literal::Bool(true))),
            "false" => Some(Type::Literal(Literal::Bool(false))),
            _ => None,
        };
        if let Some(simple) = simple {
            self.advance();
            return Ok(simple);
        }
        if word == "fn" {
            if self.gives {
                return self.fail(
                    "a function type that a function gives is written in parentheses, \
                     as `(fn(): null)`: the row after one written bare could be either's",
                );
            }
            self.advance();
            return self.function_type();
        }
        let path = self.path()?;
        let args = if self.eat("<") {
            self.list(">", Self::ty)?
        } else {
            Vec::new()
        };
        let of = |make: fn(Type) -> Type, mut args: Vec<Type>| {
            (args.len() == 1).then(|| make(args.remove(0)))
        };
        let built_in = match path.0.as_slice() {
            [name] if name == "List" => of(ty::list, args.clone()),
            [name] if name == "Cell" => of(ty::cell, args.clone()),
            [name] if name == "Iterator" => of(ty::iterator, args.clone()),
            _ => None,
        };
        Ok(built_in.unwrap_or(Type::Named(path, args)))
    }

    /// A function type, after its `fn`.
    fn function_type(&mut self) -> Parsed<Type> {
        let signature = self.signature()?;
        if !matches!(signature.receiver, Receiver::Absent) {
            return self.fail("only a member of a record type takes `self`");
        }
        Ok(Self::function_of(signature))
    }

    fn function_of(signature: Signature) -> Type {
        let function = Type::Function {
            params: signature.params,
            ret: Box::new(signature.ret),
            row: signature.row,
        };
        if signature.type_params.is_empty() {
            return function;
        }
        let params = signature.type_params.iter();
        ty::generic(
            params.map(|param| (param.name.as_str(), param.bound.clone())),
            function,
        )
    }

    /// Type parameters, parameters, what is given, and a row.
    fn signature(&mut self) -> Parsed<Signature> {
        let type_params = self.type_params()?;
        self.expect("(")?;
        let mut receiver = Receiver::Absent;
        if self.eat_word("self") {
            receiver = if self.eat(":") {
                Receiver::Needs(self.ty()?)
            } else {
                Receiver::Unstated
            };
            if !self.at(")") {
                self.expect(",")?;
            }
        }
        let params = self.list(")", |parser| {
            let Some(name) = parser.label() else {
                return parser.fail("a parameter in a type is named, as `count: int`");
            };
            Ok((name, parser.ty()?))
        })?;
        let (names, types) = params.into_iter().unzip();
        let params = Members::named(names, types);
        let ret = if self.eat(":") {
            self.given()?
        } else {
            Type::Null
        };
        Ok(Signature {
            type_params,
            receiver,
            params,
            ret,
            row: self.row()?,
        })
    }

    /// `<T, U: Bound>`, or nothing.
    pub(super) fn type_params(&mut self) -> Parsed<Vec<TypeParam>> {
        if !self.eat("<") {
            return Ok(Vec::new());
        }
        self.list(">", |parser| {
            let name = parser.name()?;
            let bound = if parser.eat(":") {
                Some(parser.ty()?)
            } else {
                None
            };
            Ok(TypeParam { name, bound })
        })
    }

    /// `with` and the abilities after it, or nothing.
    pub(super) fn row(&mut self) -> Parsed<Vec<Path>> {
        if !self.eat_word("with") {
            return Ok(Vec::new());
        }
        let mut row = vec![self.path()?];
        // A comma also separates what the row is written in: a name and
        // a colon after one begin a parameter or a member.
        while self.at(",") && self.names_ability() {
            self.advance();
            row.push(self.path()?);
        }
        Ok(row)
    }

    /// Whether what follows the next token is an ability and not the
    /// start of a parameter.
    fn names_ability(&self) -> bool {
        let mut ahead = 1;
        if !matches!(self.peek_at(ahead), Token::Word(_)) {
            return false;
        }
        while matches!(self.peek_at(ahead + 1), Token::Mark(".")) {
            ahead += 2;
        }
        !matches!(self.peek_at(ahead + 1), Token::Mark(":" | "(" | "<"))
    }

    /// A record type, from its brace.
    fn record_type(&mut self) -> Parsed<Type> {
        // Only the record a definition is written as is that definition.
        let own = self.own.take();
        self.expect("{")?;
        let members = self.list("}", |parser| parser.member_type(own.as_ref()))?;
        Ok(Type::Record(members))
    }

    fn member_type(&mut self, own: Option<&Type>) -> Parsed<(Key, Type)> {
        let key = if self.eat("[") {
            let path = self.path()?;
            self.expect("]")?;
            Key::Ability(path)
        } else {
            Key::Field(self.name()?)
        };
        if self.eat(":") {
            return Ok((key, self.ty()?));
        }
        if !self.at("(") && !self.at("<") {
            return self.unexpected("`:` and a type, or parameters");
        }
        let mut signature = self.signature()?;
        let needs = match std::mem::replace(&mut signature.receiver, Receiver::Absent) {
            Receiver::Absent => return Ok((key, Self::function_of(signature))),
            Receiver::Needs(needs) => needs,
            Receiver::Unstated => match own {
                Some(own) => own.clone(),
                None => {
                    return self.fail(
                        "a method of a record type with no name says what it needs: `self: Type`",
                    );
                }
            },
        };
        Ok((key, ty::method(needs, Self::function_of(signature))))
    }
}
