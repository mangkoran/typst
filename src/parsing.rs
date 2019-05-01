//! Tokenization and parsing of source code into syntax trees.

use std::collections::HashMap;
use std::iter::Peekable;
use std::mem::swap;
use std::str::CharIndices;

use unicode_xid::UnicodeXID;

use crate::syntax::*;
use crate::func::{ParseContext, Scope};


/// An iterator over the tokens of source code.
#[derive(Debug, Clone)]
pub struct Tokens<'s> {
    source: &'s str,
    chars: PeekableChars<'s>,
    state: TokensState,
    stack: Vec<TokensState>,
}

/// The state the tokenizer is in.
#[derive(Debug, Clone, PartialEq)]
enum TokensState {
    /// The base state if there is nothing special we are in.
    Body,
    /// Inside a function header. Here colons and equal signs get parsed
    /// as distinct tokens rather than text.
    Function,
    /// We expect either the end of the function or the beginning of the body.
    MaybeBody,
}

impl<'s> Tokens<'s> {
    /// Create a new token stream from text.
    #[inline]
    pub fn new(source: &'s str) -> Tokens<'s> {
        Tokens {
            source,
            chars: PeekableChars::new(source),
            state: TokensState::Body,
            stack: vec![],
        }
    }

    /// Advance the iterator by one step.
    fn advance(&mut self) {
        self.chars.next();
    }

    /// Switch to the given state.
    fn switch(&mut self, mut state: TokensState) {
        swap(&mut state, &mut self.state);
        self.stack.push(state);
    }

    /// Go back to the top-of-stack state.
    fn unswitch(&mut self) {
         self.state = self.stack.pop().unwrap_or(TokensState::Body);
    }

    /// Advance and return the given token.
    fn consumed(&mut self, token: Token<'s>) -> Token<'s> {
        self.advance();
        token
    }

    /// Returns a word containing the string bounded by the given indices.
    fn text(&self, start: usize, end: usize) -> Token<'s> {
        Token::Text(&self.source[start .. end])
    }
}

impl<'s> Iterator for Tokens<'s> {
    type Item = Token<'s>;

    /// Advance the iterator, return the next token or nothing.
    fn next(&mut self) -> Option<Token<'s>> {
        use TokensState as TS;

        // Function maybe has a body
        if self.state == TS::MaybeBody {
            if self.chars.peek()?.1 == '[' {
                self.state = TS::Body;
                return Some(self.consumed(Token::LeftBracket));
            } else {
                self.unswitch();
            }
        }

        // Take the next char and peek at the one behind.
        let (next_pos, next) = self.chars.next()?;
        let afterwards = self.chars.peek().map(|p| p.1);

        Some(match next {
            // Special characters
            '[' => {
                self.switch(TS::Function);
                Token::LeftBracket
            },
            ']' => {
                if self.state == TS::Function {
                    self.state = TS::MaybeBody;
                }
                Token::RightBracket
            },
            '$' => Token::Dollar,
            '#' => Token::Hashtag,

            // Whitespace
            ' ' | '\t' => {
                while let Some((_, c)) = self.chars.peek() {
                    match c {
                        ' ' | '\t' => self.advance(),
                        _ => break,
                    }
                }
                Token::Space
            }

            // Context sensitive operators in headers
            ':' if self.state == TS::Function => Token::Colon,
            '=' if self.state == TS::Function => Token::Equals,

            // Double star/underscore in bodies
            '*' if self.state == TS::Body && afterwards == Some('*')
                => self.consumed(Token::DoubleStar),
            '_' if self.state == TS::Body && afterwards == Some('_')
                => self.consumed(Token::DoubleUnderscore),

            // Newlines
            '\r' if afterwards == Some('\n') => self.consumed(Token::Newline),
            c if is_newline_char(c) => Token::Newline,

            // Escaping
            '\\' => {
                if let Some((index, c)) = self.chars.peek() {
                    let escapable = match c {
                        '[' | ']' | '$' | '#' | '\\' | '*' | '_' => true,
                        _ => false,
                    };

                    if escapable {
                        self.advance();
                        return Some(self.text(index, index + c.len_utf8()));
                    }
                }

                Token::Text("\\")
            },

            // Normal text
            _ => {
                // Find out when the word ends.
                let mut end = (next_pos, next);
                while let Some((index, c)) = self.chars.peek() {
                    // Whether the next token is still from the next or not.
                    let continues = match c {
                        '[' | ']' | '$' | '#' | '\\' => false,
                        ':' | '=' if self.state == TS::Function => false,

                        '*' if self.state == TS::Body
                             => self.chars.peek_second().map(|p| p.1) != Some('*'),
                        '_' if self.state == TS::Body
                             => self.chars.peek_second().map(|p| p.1) != Some('_'),

                        ' ' | '\t' => false,
                        c if is_newline_char(c) => false,

                        _ => true,
                    };

                    if !continues {
                        break;
                    }

                    end = (index, c);
                    self.advance();
                }

                let end_pos = end.0 + end.1.len_utf8();
                self.text(next_pos, end_pos)
            },
        })
    }
}

/// Whether this character is a newline (or starts one).
fn is_newline_char(character: char) -> bool {
    match character {
        '\n' | '\r' | '\u{000c}' | '\u{0085}' | '\u{2028}' | '\u{2029}' => true,
        _ => false,
    }
}

/// A index + char iterator with double lookahead.
#[derive(Debug, Clone)]
struct PeekableChars<'s> {
    chars: CharIndices<'s>,
    peek1: Option<Option<(usize, char)>>,
    peek2: Option<Option<(usize, char)>>,
}

impl<'s> PeekableChars<'s> {
    /// Create a new iterator from a string.
    fn new(string: &'s str) -> PeekableChars<'s> {
        PeekableChars {
            chars: string.char_indices(),
            peek1: None,
            peek2: None,
        }
    }

    /// Peek at the next element.
    fn peek(&mut self) -> Option<(usize, char)> {
        let iter = &mut self.chars;
        *self.peek1.get_or_insert_with(|| iter.next())
    }

    /// Peek at the element after the next element.
    fn peek_second(&mut self) -> Option<(usize, char)> {
        match self.peek2 {
            Some(peeked) => peeked,
            None => {
                self.peek();
                let next = self.chars.next();
                self.peek2 = Some(next);
                next
            }
        }
    }
}

impl Iterator for PeekableChars<'_> {
    type Item = (usize, char);

    fn next(&mut self) -> Option<(usize, char)> {
        match self.peek1.take() {
            Some(value) => {
                self.peek1 = self.peek2.take();
                value
            },
            None => self.chars.next(),
        }
    }
}

/// Transforms token streams to syntax trees.
pub struct Parser<'s, 't> {
    tokens: &'s mut BodyTokens<'t>,
    scope: &'s Scope,
    state: ParserState,
    tree: SyntaxTree,
}

/// The state the parser is in.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
enum ParserState {
    /// The base state of the parser.
    Body,
    /// We saw one newline already.
    FirstNewline,
    /// We wrote a newline.
    WroteNewline,
}

impl<'s, 't> Parser<'s, 't> {
    /// Create a new parser from a stream of tokens and a scope of functions.
    #[inline]
    pub fn new(tokens: &'s mut BodyTokens<'t>, scope: &'s Scope) -> Parser<'s, 't> {
        Parser {
            tokens,
            scope,
            state: ParserState::Body,
            tree: SyntaxTree::new(),
        }
    }

    /// Parse the source into an abstract syntax tree.
    pub fn parse(mut self) -> ParseResult<SyntaxTree> {
        use ParserState as PS;

        while let Some(&token) = self.tokens.peek() {
            // Skip over comments.
            if token == Token::Hashtag {
                self.skip_while(|&t| t != Token::Newline);
                self.advance();
            }

            // Handles all the states.
            match self.state {
                PS::FirstNewline => match token {
                    Token::Newline => {
                        self.append_consumed(Node::Newline);
                        self.switch(PS::WroteNewline);
                    },
                    Token::Space => self.append_space_consumed(),
                    _ => {
                        self.append_space();
                        self.switch(PS::Body);
                    },
                }

                PS::WroteNewline => match token {
                    Token::Newline | Token::Space => self.append_space_consumed(),
                    _ => self.switch(PS::Body),
                }

                PS::Body => match token {
                    // Whitespace
                    Token::Space => self.append_space_consumed(),
                    Token::Newline => {
                        self.advance();
                        self.switch(PS::FirstNewline);
                    },

                    // Text
                    Token::Text(word) => self.append_consumed(Node::Text(word.to_owned())),

                    // Functions
                    Token::LeftBracket => self.parse_function()?,
                    Token::RightBracket => {
                        return Err(ParseError::new("unexpected closing bracket"));
                    },

                    // Modifiers
                    Token::DoubleUnderscore => self.append_consumed(Node::ToggleItalics),
                    Token::DoubleStar => self.append_consumed(Node::ToggleBold),
                    Token::Dollar => self.append_consumed(Node::ToggleMath),

                    // Should not happen
                    Token::Colon | Token::Equals | Token::Hashtag => unreachable!(),
                },
            }
        }

        Ok(self.tree)
    }

    /// Parse a function from the current position.
    fn parse_function(&mut self) -> ParseResult<()> {
        // This should only be called if a left bracket was seen.
        debug_assert!(self.tokens.next() == Some(Token::LeftBracket));

        // The next token should be the name of the function.
        let name = match self.tokens.next() {
            Some(Token::Text(word)) => {
                if is_identifier(word) {
                    Ok(word.to_owned())
                } else {
                    Err(ParseError::new("invalid identifier"))
                }
            },
            _ => Err(ParseError::new("expected identifier")),
        }?;

        // Now the header should be closed.
        if self.tokens.next() != Some(Token::RightBracket) {
            return Err(ParseError::new("expected closing bracket"));
        }

        // Store the header information of the function invocation.
        let header = FuncHeader {
            name,
            args: vec![],
            kwargs: HashMap::new(),
        };

        // Whether the function has a body.
        let has_body = self.tokens.peek() == Some(&Token::LeftBracket);
        if has_body {
            self.advance();
        }

        // Now we want to parse this function dynamically.
        let parser = self.scope.get_parser(&header.name)
            .ok_or_else(|| ParseError::new(format!("unknown function: '{}'", &header.name)))?;

        // Do the parsing dependent on whether the function has a body.
        let body = if has_body {
            self.tokens.start();

            let body = parser(ParseContext {
                header: &header,
                tokens: Some(&mut self.tokens),
                scope: &self.scope,
            })?;

            self.tokens.finish();

            // Now the body should be closed.
            if self.tokens.next() != Some(Token::RightBracket) {
                return Err(ParseError::new("expected closing bracket"));
            }

            body
        } else {
            parser(ParseContext {
                header: &header,
                tokens: None,
                scope: &self.scope,
            })?
        };

        // Finally this function is parsed to the end.
        self.append(Node::Func(FuncCall {
            header,
            body,
        }));

        Ok(self.switch(ParserState::Body))
    }

    /// Advance the iterator by one step.
    fn advance(&mut self) {
        self.tokens.next();
    }

    /// Switch the state.
    fn switch(&mut self, state: ParserState) {
        self.state = state;
    }

    /// Append a node to the tree.
    fn append(&mut self, node: Node) {
        self.tree.nodes.push(node);
    }

    /// Append a space if there is not one already.
    fn append_space(&mut self) {
        if self.tree.nodes.last() != Some(&Node::Space) {
            self.append(Node::Space);
        }
    }

    /// Advance and return the given node.
    fn append_consumed(&mut self, node: Node) {
        self.advance();
        self.append(node);
    }

    /// Advance and append a space if there is not one already.
    fn append_space_consumed(&mut self) {
        self.advance();
        self.append_space();
    }

    /// Skip tokens until the condition is met.
    fn skip_while<F>(&mut self, f: F) where F: Fn(&Token) -> bool {
        while let Some(token) = self.tokens.peek() {
            if !f(token) {
                break;
            }
            self.advance();
        }
    }
}

/// Whether this word is a valid unicode identifier.
fn is_identifier(string: &str) -> bool {
    let mut chars = string.chars();

    match chars.next() {
        Some(c) if !UnicodeXID::is_xid_start(c) => return false,
        None => return false,
        _ => (),
    }

    while let Some(c) = chars.next() {
        if !UnicodeXID::is_xid_continue(c) {
            return false;
        }
    }

    true
}

/// A token iterator that iterates over exactly one body.
///
/// This iterator wraps [`Tokens`] and yields exactly the tokens of one
/// function body or the complete top-level body and stops then.
#[derive(Debug, Clone)]
pub struct BodyTokens<'s> {
    tokens: Peekable<Tokens<'s>>,
    parens: Vec<u32>,
    blocked: bool,
}

impl<'s> BodyTokens<'s> {
    /// Create a new iterator over text.
    #[inline]
    pub fn new(source: &'s str) -> BodyTokens<'s> {
        BodyTokens::from_tokens(Tokens::new(source))
    }

    /// Create a new iterator operating over an existing one.
    #[inline]
    pub fn from_tokens(tokens: Tokens<'s>) -> BodyTokens<'s> {
        BodyTokens {
            tokens: tokens.peekable(),
            parens: vec![],
            blocked: false,
        }
    }

    /// Peek at the next token.
    #[inline]
    pub fn peek(&mut self) -> Option<&Token<'s>> {
        if self.blocked {
            return None;
        }

        let token = self.tokens.peek();
        if token == Some(&Token::RightBracket) && self.parens.last() == Some(&0) {
            return None;
        }

        token
    }

    /// Start a new substream of tokens.
    fn start(&mut self) {
        self.parens.push(0);
    }

    /// Finish a substream of tokens.
    fn finish(&mut self) {
        self.blocked = false;
        self.parens.pop().unwrap();
    }
}

impl<'s> Iterator for BodyTokens<'s> {
    type Item = Token<'s>;

    fn next(&mut self) -> Option<Token<'s>> {
        if self.blocked {
            return None;
        }

        let token = self.tokens.peek();
        match token {
            Some(Token::RightBracket) => {
                match self.parens.last_mut() {
                    Some(&mut 0) => {
                        self.blocked = true;
                        return None
                    },
                    Some(top) => *top -= 1,
                    None => {}
                }
            },
            Some(Token::LeftBracket) => {
                if let Some(top) = self.parens.last_mut() {
                    *top += 1;
                }
            }
            _ => {}
        };
        self.tokens.next()
    }
}

/// The error type for parsing.
pub struct ParseError(String);

/// The result type for parsing.
pub type ParseResult<T> = Result<T, ParseError>;

impl ParseError {
    fn new<S: Into<String>>(message: S) -> ParseError {
        ParseError(message.into())
    }
}

error_type! {
    err: ParseError,
    show: f => f.write_str(&err.0),
}


#[cfg(test)]
mod token_tests {
    use super::*;
    use Token::{Space as S, Newline as N, LeftBracket as L, RightBracket as R,
                Colon as C, Equals as E, DoubleUnderscore as DU, DoubleStar as DS,
                Dollar as D, Hashtag as H, Text as T};

    /// Test if the source code tokenizes to the tokens.
    fn test(src: &str, tokens: Vec<Token>) {
        assert_eq!(Tokens::new(src).collect::<Vec<_>>(), tokens);
    }

    /// Tokenizes the basic building blocks.
    #[test]
    fn tokenize_base() {
        test("", vec![]);
        test("Hallo", vec![T("Hallo")]);
        test("[", vec![L]);
        test("]", vec![R]);
        test("$", vec![D]);
        test("#", vec![H]);
        test("**", vec![DS]);
        test("__", vec![DU]);
        test("\n", vec![N]);
    }

    /// This test looks if LF- and CRLF-style newlines get both identified correctly
    #[test]
    fn tokenize_whitespace_newlines() {
        test(" \t", vec![S]);
        test("First line\r\nSecond line\nThird line\n",
             vec![T("First"), S, T("line"), N, T("Second"), S, T("line"), N,
                  T("Third"), S, T("line"), N]);
        test("Hello \n ", vec![T("Hello"), S, N, S]);
        test("Dense\nTimes", vec![T("Dense"), N, T("Times")]);
    }

    /// Tests if escaping with backslash works as it should.
    #[test]
    fn tokenize_escape() {
        test(r"\[", vec![T("[")]);
        test(r"\]", vec![T("]")]);
        test(r"\#", vec![T("#")]);
        test(r"\$", vec![T("$")]);
        test(r"\**", vec![T("*"), T("*")]);
        test(r"\*", vec![T("*")]);
        test(r"\__", vec![T("_"), T("_")]);
        test(r"\_", vec![T("_")]);
        test(r"\hello", vec![T("\\"), T("hello")]);
    }

    /// Tokenizes some more realistic examples.
    #[test]
    fn tokenize_examples() {
        test(r"
            [function][
                Test [italic][example]!
            ]
        ", vec![
            N, S, L, T("function"), R, L, N, S, T("Test"), S, L, T("italic"), R, L,
            T("example"), R, T("!"), N, S, R, N, S
        ]);

        test(r"
            [page: size=A4]
            [font: size=12pt]

            Das ist ein Beispielsatz mit **fetter** Schrift.
        ", vec![
            N, S, L, T("page"), C, S, T("size"), E, T("A4"), R, N, S,
            L, T("font"), C, S, T("size"), E, T("12pt"), R, N, N, S,
            T("Das"), S, T("ist"), S, T("ein"), S, T("Beispielsatz"), S, T("mit"), S,
            DS, T("fetter"), DS, S, T("Schrift."), N, S
        ]);
    }

    /// This test checks whether the colon and equals symbols get parsed correctly
    /// depending on the context: Either in a function header or in a body.
    #[test]
    fn tokenize_symbols_context() {
        test("[func: key=value][Answer: 7]",
             vec![L, T("func"), C, S, T("key"), E, T("value"), R, L,
                  T("Answer:"), S, T("7"), R]);
        test("[[n: k=v]:x][:[=]]:=",
             vec![L, L, T("n"), C, S, T("k"), E, T("v"), R, C, T("x"), R,
                  L, T(":"), L, E, R, R, T(":=")]);
        test("[func: __key__=value]",
             vec![L, T("func"), C, S, T("__key__"), E, T("value"), R]);
    }

    /// This test has a special look at the double underscore syntax, because
    /// per Unicode standard they are not separate words and thus harder to parse
    /// than the stars.
    #[test]
    fn tokenize_double_underscore() {
        test("he__llo__world_ _ __ Now this_ is__ special!",
             vec![T("he"), DU, T("llo"), DU, T("world_"), S, T("_"), S, DU, S, T("Now"), S,
                  T("this_"), S, T("is"), DU, S, T("special!")]);
    }

    /// This test is for checking if non-ASCII characters get parsed correctly.
    #[test]
    fn tokenize_unicode() {
        test("[document][Hello 🌍!]",
             vec![L, T("document"), R, L, T("Hello"), S, T("🌍!"), R]);
        test("[f]⺐.", vec![L, T("f"), R, T("⺐.")]);
    }
}


#[cfg(test)]
mod parse_tests {
    use super::*;
    use funcs::*;
    use crate::func::{Function, Scope};
    use Node::{Space as S, Newline as N, Func as F};

    /// Two test functions, one which parses it's body as another syntax tree
    /// and another one which does not expect a body.
    mod funcs {
        use super::*;

        /// A testing function which just parses it's body into a syntax tree.
        #[derive(Debug, PartialEq)]
        pub struct TreeFn(pub SyntaxTree);

        impl Function for TreeFn {
            fn parse(context: ParseContext) -> ParseResult<Self> where Self: Sized {
                if let Some(tokens) = context.tokens {
                    Parser::new(tokens, context.scope).parse().map(|tree| TreeFn(tree))
                } else {
                    Err(ParseError::new("expected body for tree fn"))
                }
            }
            fn typeset(&self, _header: &FuncHeader) -> Option<Expression> { None }
        }

        /// A testing function without a body.
        #[derive(Debug, PartialEq)]
        pub struct BodylessFn;

        impl Function for BodylessFn {
            fn parse(context: ParseContext) -> ParseResult<Self> where Self: Sized {
                if context.tokens.is_none() {
                    Ok(BodylessFn)
                } else {
                    Err(ParseError::new("unexpected body for bodyless fn"))
                }
            }
            fn typeset(&self, _header: &FuncHeader) -> Option<Expression> { None }
        }
    }

    /// Test if the source code parses into the syntax tree.
    fn test(src: &str, tree: SyntaxTree) {
        assert_eq!(parse(src, &Scope::new()).unwrap(), tree);
    }

    /// Test with a scope containing function definitions.
    fn test_scoped(scope: &Scope, src: &str, tree: SyntaxTree) {
        assert_eq!(parse(src, &scope).unwrap(), tree);
    }

    /// Test if the source parses into the error.
    fn test_err(src: &str, err: &str) {
        assert_eq!(parse(src, &Scope::new()).unwrap_err().to_string(), err);
    }

    /// Test with a scope if the source parses into the error.
    fn test_err_scoped(scope: &Scope, src: &str, err: &str) {
        assert_eq!(parse(src, &scope).unwrap_err().to_string(), err);
    }

    /// Parse the source code with the given scope.
    fn parse(src: &str, scope: &Scope) -> ParseResult<SyntaxTree> {
        let mut tokens = BodyTokens::new(src);
        Parser::new(&mut tokens, scope).parse()
    }

    /// Create a text node.
    #[allow(non_snake_case)]
    fn T(s: &str) -> Node { Node::Text(s.to_owned()) }

    /// Shortcut macro to create a syntax tree.
    /// Is `vec`-like and the elements are the nodes.
    macro_rules! tree {
        ($($x:expr),*) => (
            SyntaxTree { nodes: vec![$($x),*] }
        );
        ($($x:expr,)*) => (tree![$($x),*])
    }

    /// Shortcut macro to create a function.
    macro_rules! func {
        (name => $name:expr, body => None $(,)*) => {
            func!(@$name, Box::new(BodylessFn))
        };
        (name => $name:expr, body => $tree:expr $(,)*) => {
            func!(@$name, Box::new(TreeFn($tree)))
        };
        (@$name:expr, $body:expr) => {
            FuncCall {
                header: FuncHeader {
                    name: $name.to_string(),
                    args: vec![],
                    kwargs: HashMap::new(),
                },
                body: $body,
            }
        }
    }

    /// Parse the basic cases.
    #[test]
    fn parse_base() {
        test("", tree! []);
        test("Hello World!", tree! [ T("Hello"), S, T("World!") ]);
    }

    /// Test whether newlines generate the correct whitespace.
    #[test]
    fn parse_newlines_whitespace() {
        test("Hello\nWorld", tree! [ T("Hello"), S, T("World") ]);
        test("Hello \n World", tree! [ T("Hello"), S, T("World") ]);
        test("Hello\n\nWorld", tree! [ T("Hello"), N, T("World") ]);
        test("Hello \n\nWorld", tree! [ T("Hello"), S, N, T("World") ]);
        test("Hello\n\n  World", tree! [ T("Hello"), N, S, T("World") ]);
        test("Hello \n \n \n  World", tree! [ T("Hello"), S, N, S, T("World") ]);
        test("Hello\n \n\n  World", tree! [ T("Hello"), S, N, S, T("World") ]);
    }

    /// Parse things dealing with functions.
    #[test]
    fn parse_functions() {
        let mut scope = Scope::new();
        scope.add::<BodylessFn>("test");
        scope.add::<BodylessFn>("end");
        scope.add::<TreeFn>("modifier");
        scope.add::<TreeFn>("func");

        test_scoped(&scope,"[test]", tree! [ F(func! { name => "test", body => None }) ]);
        test_scoped(&scope, "This is an [modifier][example] of a function invocation.", tree! [
            T("This"), S, T("is"), S, T("an"), S,
            F(func! { name => "modifier", body => tree! [ T("example") ] }), S,
            T("of"), S, T("a"), S, T("function"), S, T("invocation.")
        ]);
        test_scoped(&scope, "[func][Hello][modifier][Here][end]",  tree! [
            F(func! {
                name => "func",
                body => tree! [ T("Hello") ],
            }),
            F(func! {
                name => "modifier",
                body => tree! [ T("Here") ],
            }),
            F(func! {
                name => "end",
                body => None,
            }),
        ]);
        test_scoped(&scope, "[func][]", tree! [
            F(func! {
                name => "func",
                body => tree! [],
            })
        ]);
        test_scoped(&scope, "[modifier][[func][call]] outside", tree! [
            F(func! {
                name => "modifier",
                body => tree! [
                    F(func! {
                        name => "func",
                        body => tree! [ T("call") ],
                    }),
                ],
            }),
            S, T("outside")
        ]);
    }

    /// Tests if the parser handles non-ASCII stuff correctly.
    #[test]
    fn parse_unicode() {
        let mut scope = Scope::new();
        scope.add::<BodylessFn>("func");
        scope.add::<TreeFn>("bold");
        test_scoped(&scope, "[func] ⺐.", tree! [
            F(func! {
                name => "func",
                body => None,
            }),
            S, T("⺐.")
        ]);
        test_scoped(&scope, "[bold][Hello 🌍!]", tree! [
            F(func! {
                name => "bold",
                body => tree! [ T("Hello"), S, T("🌍!") ],
            })
        ]);
    }

    /// Tests whether errors get reported correctly.
    #[test]
    fn parse_errors() {
        let mut scope = Scope::new();
        scope.add::<TreeFn>("hello");

        test_err("No functions here]", "unexpected closing bracket");
        test_err_scoped(&scope, "[hello][world", "expected closing bracket");
        test_err("[hello world", "expected closing bracket");
        test_err("[ no-name][Why?]", "expected identifier");
    }
}
