use std::fmt;
use std::fmt::Write;
use std::slice::Iter;
use std::str::Chars;
use std::vec::IntoIter;

use TokenKind::*;

use crate::log;

pub const EOF_CHAR: char = '\0';
pub const EOF_TOKEN: Token = Token {
    start: 0,
    len: 0,
    line_num: 0,
    kind: TokenKind::EOF,
};

pub struct Tokens {
    iter: IntoIter<Token>,
}

impl Tokens {
    pub fn make(data: Vec<Token>) -> Tokens {
        Tokens { iter: data.into_iter() }
    }
    pub fn next(&mut self) -> Token {
        self.iter.next().unwrap_or(EOF_TOKEN)
    }
    pub fn advance(&mut self) -> () {
        self.next();
        ()
    }
    pub fn peek(&self) -> Token {
        let peeked = self.iter.clone().next();
        peeked.unwrap_or(EOF_TOKEN)
    }
    pub fn peek_two(&self) -> (Token, Token) {
        let mut peek_iter = self.iter.clone();
        let p1 = peek_iter.next().unwrap_or(EOF_TOKEN);
        let p2 = peek_iter.next().unwrap_or(EOF_TOKEN);
        (p1, p2)
    }
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum TokenKind {
    Text,

    KeywordFn,
    KeywordReturn,
    KeywordVal,
    KeywordMut,

    LineComment,

    OpenParen,
    CloseParen,
    OpenBracket,
    CloseBracket,
    OpenBrace,
    CloseBrace,
    Colon,
    Semicolon,
    Equals,
    Dot,
    Comma,

    /// Not really a token but allows us to avoid Option<Token> everywhere
    EOF,
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.get_repr().unwrap_or("<ident>"))
    }
}

impl TokenKind {
    pub fn get_repr(&self) -> Option<&'static str> {
        match self {
            KeywordFn => Some("fn"),
            KeywordReturn => Some("return"),
            KeywordVal => Some("val"),
            KeywordMut => Some("mut"),

            OpenParen => Some("("),
            CloseParen => Some(")"),
            OpenBracket => Some("["),
            CloseBracket => Some("]"),
            OpenBrace => Some("{"),
            CloseBrace => Some("}"),
            Colon => Some(":"),
            Semicolon => Some(";"),
            Equals => Some("="),
            Dot => Some("."),
            Comma => Some(","),

            Text => None,

            LineComment => None,

            EOF => Some("<EOF>"),
        }
    }
    pub fn from_char(c: char) -> Option<TokenKind> {
        match c {
            '(' => Some(OpenParen),
            ')' => Some(CloseParen),
            '[' => Some(OpenBracket),
            ']' => Some(CloseBracket),
            '{' => Some(OpenBrace),
            '}' => Some(CloseBrace),
            ':' => Some(Colon),
            ';' => Some(Semicolon),
            '=' => Some(Equals),
            '.' => Some(Dot),
            ',' => Some(Comma),
            _ => None
        }
    }
    pub fn keyword_from_str(str: &str) -> Option<TokenKind> {
        match str {
            "fn" => Some(KeywordFn),
            "return" => Some(KeywordReturn),
            "val" => Some(KeywordVal),
            "mut" => Some(KeywordMut),
            _ => None
        }
    }
    pub fn is_keyword(&self) -> bool {
        match self {
            KeywordFn => true,
            KeywordReturn => true,
            KeywordVal => true,
            KeywordMut => true,
            _ => false
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub struct Token {
    pub start: usize,
    pub len: usize,
    pub line_num: usize,
    pub kind: TokenKind,
}

impl Token {
    pub fn make(kind: TokenKind, line_num: usize, start: usize, len: usize) -> Token {
        Token {
            start,
            len,
            line_num,
            kind,
        }
    }
}

pub struct Lexer<'a> {
    content: Chars<'a>,
    pub line_index: usize,
    pub pos: usize,
}

impl Lexer<'_> {
    pub fn make(input: &str) -> Lexer {
        Lexer {
            content: input.chars(),
            line_index: 0,
            pos: 0,
        }
    }
    pub fn next(&mut self) -> char {
        self.pos += 1;
        let c = self.content.next().unwrap_or(EOF_CHAR);
        if c == '\n' {
            self.line_index += 1;
        }
        if c == '\r' && self.peek() == '\n' {
            self.content.next();
            self.pos += 1;
        }
        c
    }
    pub fn next_with_pos(&mut self) -> (char, usize) {
        let old_pos = self.pos;
        (self.next(), old_pos)
    }
    pub fn peek(&self) -> char {
        self.content.clone().next().unwrap_or(EOF_CHAR)
    }
    pub fn peek_two(&self) -> (char, char) {
        let mut peek_iter = self.content.clone();
        (peek_iter.next().unwrap_or(EOF_CHAR), peek_iter.next().unwrap_or(EOF_CHAR))
    }
    pub fn peek_with_pos(&self) -> (char, usize) {
        (self.peek(), self.pos)
    }
    pub fn advance(&mut self) -> () {
        self.next();
        ()
    }
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn eat_token(lexer: &mut Lexer) -> Option<Token> {
    let mut tok_buf = String::new();
    let mut tok_len = 0;
    loop {
        let (c, n) = lexer.peek_with_pos();
        log::verbose(&format!("LEX line={} char={} '{}'", lexer.line_index, n, c));
        if c == EOF_CHAR {
            break None;
        }
        if let Some(single_char_tok) = TokenKind::from_char(c) {
            if !tok_buf.is_empty() {
                break Some(Token::make(TokenKind::Text, lexer.line_index, n - tok_len, tok_len));
            } else {
                lexer.advance();
                break Some(Token::make(single_char_tok, lexer.line_index, n, 1));
            }
        }
        if c.is_whitespace() {
            if !tok_buf.is_empty() {
                lexer.advance();
                if let Some(tok) = TokenKind::keyword_from_str(&tok_buf) {
                    break Some(Token::make(tok, lexer.line_index, n - tok_len, tok_len));
                } else {
                    break Some(Token::make(TokenKind::Text, lexer.line_index, n - tok_len, tok_len));
                }
            }
        }
        if (tok_buf.is_empty() && is_ident_start(c)) || is_ident_char(c) {
            tok_len += 1;
            tok_buf.push(c);
        } else if let Some(tok) = TokenKind::keyword_from_str(&tok_buf) {
            lexer.advance();
            break Some(Token::make(tok, lexer.line_index, n - tok_len, tok_len));
        }
        lexer.advance();
    }
}

pub fn tokenize(lexer: &mut Lexer) -> Vec<Token> {
    let mut tokens = Vec::with_capacity(1024);
    while let Some(tok) = eat_token(lexer) {
        tokens.push(tok);
    }
    tokens
}