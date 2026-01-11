//! JavaScript lexer

use crate::error::{JSError, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Token<'a> {
    Undefined,
    Null,
    True,
    False,
    Number(f64),
    String(&'a str),
    Identifier(&'a str),
    Let,
    Const,
    Var,
    Function,
    Return,
    If,
    Else,
    While,
    For,
    Break,
    Continue,
    Throw,
    Try,
    Catch,
    Finally,
    Match,
    Perform,
    Handle,
    With,
    New,
    This,
    TypeOf,
    InstanceOf,
    In,
    Of,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    StarStar,
    PlusPlus,
    MinusMinus,
    EqEq,
    EqEqEq,
    BangEq,
    BangEqEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    AmpAmp,
    PipePipe,
    Bang,
    Question,
    QuestionQuestion,
    Amp,
    Pipe,
    Caret,
    Tilde,
    LtLt,
    GtGt,
    GtGtGt,
    Eq,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    AmpEq,
    PipeEq,
    CaretEq,
    LtLtEq,
    GtGtEq,
    GtGtGtEq,
    AmpAmpEq,
    PipePipeEq,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Dot,
    Colon,
    Semicolon,
    Arrow,      // =>
    ThinArrow,  // ->
    Spread,
    Eof,
}

pub struct Lexer<'a> {
    source: &'a str,
    chars: std::str::Chars<'a>,
    current: Option<char>,
    line: u32,
    column: u32,
    start_line: u32,
    start_column: u32,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Self {
        let mut chars = source.chars();
        let current = chars.next();
        Self {
            source,
            chars,
            current,
            line: 1,
            column: 1,
            start_line: 1,
            start_column: 1,
        }
    }

    pub fn line(&self) -> u32 {
        self.start_line
    }

    pub fn column(&self) -> u32 {
        self.start_column
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.current;
        self.current = self.chars.next();
        if let Some(ch) = c {
            if ch == '\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
        }
        c
    }

    fn peek(&self) -> Option<char> {
        self.current
    }

    fn peek_next(&self) -> Option<char> {
        self.chars.clone().next()
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek() {
            match c {
                ' ' | '\t' | '\r' | '\n' => {
                    self.advance();
                }
                '/' if self.peek_next() == Some('/') => {
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.advance();
                    }
                }
                '/' if self.peek_next() == Some('*') => {
                    self.advance();
                    self.advance();
                    while let Some(c) = self.peek() {
                        if c == '*' && self.peek_next() == Some('/') {
                            self.advance();
                            self.advance();
                            break;
                        }
                        self.advance();
                    }
                }
                _ => break,
            }
        }
    }

    fn number(&mut self, first: char) -> Result<Token<'a>> {
        let mut value: f64 = (first as u8 - b'0') as f64;
        let mut is_float = false;

        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                value = value * 10.0 + (c as u8 - b'0') as f64;
                self.advance();
            } else if c == '.' && !is_float {
                is_float = true;
                self.advance();
                let mut decimal = 0.1;
                while let Some(c) = self.peek() {
                    if c.is_ascii_digit() {
                        value += (c as u8 - b'0') as f64 * decimal;
                        decimal *= 0.1;
                        self.advance();
                    } else {
                        break;
                    }
                }
            } else {
                break;
            }
        }

        Ok(Token::Number(value))
    }

    fn identifier_or_keyword(&mut self, start_pos: usize) -> Token<'a> {
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '$' {
                self.advance();
            } else {
                break;
            }
        }

        let end_pos = self.source.len()
            - self.chars.as_str().len()
            - self.current.map(|c| c.len_utf8()).unwrap_or(0);
        let text = &self.source[start_pos..end_pos];

        match text {
            "undefined" => Token::Undefined,
            "null" => Token::Null,
            "true" => Token::True,
            "false" => Token::False,
            "let" => Token::Let,
            "const" => Token::Const,
            "var" => Token::Var,
            "function" => Token::Function,
            "return" => Token::Return,
            "if" => Token::If,
            "else" => Token::Else,
            "while" => Token::While,
            "for" => Token::For,
            "break" => Token::Break,
            "continue" => Token::Continue,
            "throw" => Token::Throw,
            "try" => Token::Try,
            "catch" => Token::Catch,
            "finally" => Token::Finally,
            "match" => Token::Match,
            "perform" => Token::Perform,
            "handle" => Token::Handle,
            "with" => Token::With,
            "new" => Token::New,
            "this" => Token::This,
            "typeof" => Token::TypeOf,
            "instanceof" => Token::InstanceOf,
            "in" => Token::In,
            "of" => Token::Of,
            _ => Token::Identifier(text)
        }
    }

    fn string(&mut self, quote: char) -> Result<Token<'a>> {
        // Position of current char (first string content char)
        let start_pos = self.source.len()
            - self.chars.as_str().len()
            - self.current.map(|c| c.len_utf8()).unwrap_or(0);

        while let Some(c) = self.peek() {
            if c == quote {
                let end_pos = self.source.len()
                    - self.chars.as_str().len()
                    - self.current.map(|c| c.len_utf8()).unwrap_or(0);
                self.advance(); // consume closing quote
                return Ok(Token::String(&self.source[start_pos..end_pos]));
            }
            if c == '\\' {
                self.advance(); // skip backslash
                self.advance(); // skip escaped char
            } else if c == '\n' {
                return Err(JSError::syntax(
                    "Unterminated string",
                    self.line,
                    self.column,
                ));
            } else {
                self.advance();
            }
        }

        Err(JSError::syntax(
            "Unterminated string",
            self.line,
            self.column,
        ))
    }

    pub fn next_token(&mut self) -> Result<Token<'a>> {
        self.skip_whitespace();
        self.start_line = self.line;
        self.start_column = self.column;

        let c = match self.advance() {
            Some(c) => c,
            None => return Ok(Token::Eof),
        };

        match c {
            '(' => Ok(Token::LParen),
            ')' => Ok(Token::RParen),
            '{' => Ok(Token::LBrace),
            '}' => Ok(Token::RBrace),
            '[' => Ok(Token::LBracket),
            ']' => Ok(Token::RBracket),
            ',' => Ok(Token::Comma),
            ':' => Ok(Token::Colon),
            ';' => Ok(Token::Semicolon),
            '~' => Ok(Token::Tilde),

            '.' => {
                if self.peek() == Some('.') && self.peek_next() == Some('.') {
                    self.advance();
                    self.advance();
                    Ok(Token::Spread)
                } else {
                    Ok(Token::Dot)
                }
            }

            '+' => {
                if self.peek() == Some('+') {
                    self.advance();
                    Ok(Token::PlusPlus)
                } else if self.peek() == Some('=') {
                    self.advance();
                    Ok(Token::PlusEq)
                } else {
                    Ok(Token::Plus)
                }
            }

            '-' => {
                if self.peek() == Some('-') {
                    self.advance();
                    Ok(Token::MinusMinus)
                } else if self.peek() == Some('=') {
                    self.advance();
                    Ok(Token::MinusEq)
                } else if self.peek() == Some('>') {
                    self.advance();
                    Ok(Token::ThinArrow)
                } else {
                    Ok(Token::Minus)
                }
            }

            '*' => {
                if self.peek() == Some('*') {
                    self.advance();
                    Ok(Token::StarStar)
                } else if self.peek() == Some('=') {
                    self.advance();
                    Ok(Token::StarEq)
                } else {
                    Ok(Token::Star)
                }
            }

            '/' => {
                if self.peek() == Some('=') {
                    self.advance();
                    Ok(Token::SlashEq)
                } else {
                    Ok(Token::Slash)
                }
            }

            '%' => {
                if self.peek() == Some('=') {
                    self.advance();
                    Ok(Token::PercentEq)
                } else {
                    Ok(Token::Percent)
                }
            }

            '=' => {
                if self.peek() == Some('=') {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        Ok(Token::EqEqEq)
                    } else {
                        Ok(Token::EqEq)
                    }
                } else if self.peek() == Some('>') {
                    self.advance();
                    Ok(Token::Arrow)
                } else {
                    Ok(Token::Eq)
                }
            }

            '!' => {
                if self.peek() == Some('=') {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        Ok(Token::BangEqEq)
                    } else {
                        Ok(Token::BangEq)
                    }
                } else {
                    Ok(Token::Bang)
                }
            }

            '<' => {
                if self.peek() == Some('<') {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        Ok(Token::LtLtEq)
                    } else {
                        Ok(Token::LtLt)
                    }
                } else if self.peek() == Some('=') {
                    self.advance();
                    Ok(Token::LtEq)
                } else {
                    Ok(Token::Lt)
                }
            }

            '>' => {
                if self.peek() == Some('>') {
                    self.advance();
                    if self.peek() == Some('>') {
                        self.advance();
                        if self.peek() == Some('=') {
                            self.advance();
                            Ok(Token::GtGtGtEq)
                        } else {
                            Ok(Token::GtGtGt)
                        }
                    } else if self.peek() == Some('=') {
                        self.advance();
                        Ok(Token::GtGtEq)
                    } else {
                        Ok(Token::GtGt)
                    }
                } else if self.peek() == Some('=') {
                    self.advance();
                    Ok(Token::GtEq)
                } else {
                    Ok(Token::Gt)
                }
            }

            '&' => {
                if self.peek() == Some('&') {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        Ok(Token::AmpAmpEq)
                    } else {
                        Ok(Token::AmpAmp)
                    }
                } else if self.peek() == Some('=') {
                    self.advance();
                    Ok(Token::AmpEq)
                } else {
                    Ok(Token::Amp)
                }
            }

            '|' => {
                if self.peek() == Some('|') {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        Ok(Token::PipePipeEq)
                    } else {
                        Ok(Token::PipePipe)
                    }
                } else if self.peek() == Some('=') {
                    self.advance();
                    Ok(Token::PipeEq)
                } else {
                    Ok(Token::Pipe)
                }
            }

            '^' => {
                if self.peek() == Some('=') {
                    self.advance();
                    Ok(Token::CaretEq)
                } else {
                    Ok(Token::Caret)
                }
            }

            '?' => {
                if self.peek() == Some('?') {
                    self.advance();
                    Ok(Token::QuestionQuestion)
                } else {
                    Ok(Token::Question)
                }
            }

            '"' | '\'' => self.string(c),

            '0'..='9' => self.number(c),

            'a'..='z' | 'A'..='Z' | '_' | '$' => {
                // Position of 'c' = position of current - c.len_utf8()
                let start_pos = self.source.len()
                    - self.chars.as_str().len()
                    - self.current.map(|ch| ch.len_utf8()).unwrap_or(0)
                    - c.len_utf8();
                Ok(self.identifier_or_keyword(start_pos))
            }

            _ => Err(JSError::syntax(
                "Unexpected character",
                self.line,
                self.column,
            )),
        }
    }
}
