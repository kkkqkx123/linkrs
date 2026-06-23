//! Token definitions for the query parser
//!
//! This module defines the lexical tokens used by the parser.

use std::fmt;

use crate::core::types::Position;

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub lexeme: String,
    pub line: usize,
    pub column: usize,
}

impl Token {
    pub fn new(kind: TokenKind, lexeme: String, line: usize, column: usize) -> Self {
        Token {
            kind,
            lexeme,
            line,
            column,
        }
    }

    pub fn position(&self) -> Position {
        Position::new(self.line, self.column)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Create,
    Match,
    Return,
    Where,
    Delete,
    Update,
    Insert,
    Upsert,
    From,
    To,
    As,
    With,
    Yield,
    Go,
    Over,
    Step,
    Upto,
    Limit,
    Asc,
    Desc,
    Order,
    By,
    Skip,
    Unwind,
    Optional,
    Distinct,
    All,
    Null,
    Is,
    Not,
    And,
    Or,
    Xor,
    Contains,
    StartsWith,
    EndsWith,
    Case,
    When,
    Then,
    Else,
    End,
    Union,
    Intersect,
    SetMinus,
    Group,
    Having,
    Between,
    Admin,
    Edge,
    Edges,
    Vertex,
    Vertices,
    Tag,
    Tags,
    Index,
    Indexes,
    Lookup,
    Find,
    Path,
    Shortest,
    AllShortestPaths,
    Subgraph,
    Both,
    Out,
    In,
    Reversely,
    No,
    Overwrite,
    Show,
    Add,
    Drop,
    Remove,
    Alter,
    If,
    Exists,
    Change,
    CreateUser,
    AlterUser,
    DropUser,
    ChangePassword,
    Grant,
    Revoke,
    On,
    Of,
    Get,
    Set,
    Host,
    Hosts,
    Space,
    Spaces,
    User,
    Users,
    Password,
    Role,
    Roles,
    Locked,
    God,
    AdminRole,
    Dba,
    Guest,
    Comment,
    Charset,
    Collate,
    Collation,
    VIdType,
    PartitionNum,
    ReplicaFactor,
    Rebuild,
    Bool,
    Int,
    Int8,
    Int16,
    Int32,
    Int64,
    Float,
    Double,
    String,
    FixedString,
    Timestamp,
    Date,
    Time,
    Datetime,
    Duration,
    Geography,
    Point,
    Linestring,
    Polygon,
    List,
    Map,
    Download,
    HDFS,
    UUID,
    Configs,
    Force,
    Part,
    Parts,
    Data,
    Leader,
    Jobs,
    Job,
    Bidirect,
    Stats,
    Status,
    Recover,
    Explain,
    Profile,
    Format,
    AtomicEdge,
    Default,
    Flush,
    Compact,
    Submit,
    Ascending,
    Descending,
    Fetch,
    Prop,
    Balance,
    Stop,
    Revert,
    Use,
    Begin,
    Commit,
    Rollback,
    Transaction,
    SetList,
    Clear,
    Merge,
    Values,
    Divide,
    Rename,
    Local,
    Sessions,
    Session,
    Sample,
    Queries,
    Query,
    Kill,
    Top,
    Text,
    Search,
    KeywordVector,
    VectorLiteral(Vec<f32>),
    Client,
    Clients,
    Sign,
    Service,
    Count,
    Sum,
    Avg,
    Min,
    Max,
    NotIn,
    IsNull,
    IsNotNull,
    IsEmpty,
    IsNotEmpty,
    Outbound,
    Inbound,
    Source,
    Destination,
    Rank,
    Input,
    FindPath,
    Weight,
    Loop,
    Cycle,
    TtlDuration,
    TtlCol,
    Identifier(String),
    StringLiteral(String),
    IntegerLiteral(i64),
    FloatLiteral(f64),
    BooleanLiteral(bool),
    Plus,
    Minus,
    Star,
    Div,
    Mod,
    Exp,
    Eq,
    Assign,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Regex,
    NotOp,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Dot,
    DotDot,
    Colon,
    DoubleColon,
    Semicolon,
    QMark,
    Question,
    Pipe,
    Arrow,
    BackArrow,
    RightArrow,
    LeftArrow,
    At,
    Dollar,
    IdProp,
    TypeProp,
    SrcIdProp,
    DstIdProp,
    RankProp,
    DstRef,
    SrcRef,
    InputRef,
    Eof,
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenKind::Identifier(s) => write!(f, "{}", s),
            TokenKind::StringLiteral(s) => write!(f, "\"{}\"", s),
            TokenKind::IntegerLiteral(n) => write!(f, "{}", n),
            TokenKind::FloatLiteral(n) => write!(f, "{}", n),
            TokenKind::BooleanLiteral(b) => write!(f, "{}", b),
            _ => write!(f, "{:?}", self),
        }
    }
}

pub trait TokenKindExt {
    fn is_keyword(&self) -> bool;
    fn is_literal(&self) -> bool;
    fn is_operator(&self) -> bool;
    fn is_delimiter(&self) -> bool;
    fn is_identifier(&self) -> bool;
}

impl TokenKindExt for TokenKind {
    fn is_keyword(&self) -> bool {
        matches!(
            self,
            TokenKind::Create
                | TokenKind::Match
                | TokenKind::Return
                | TokenKind::Where
                | TokenKind::Delete
                | TokenKind::Update
                | TokenKind::Insert
                | TokenKind::Upsert
                | TokenKind::From
                | TokenKind::To
                | TokenKind::As
                | TokenKind::With
                | TokenKind::Yield
                | TokenKind::Go
                | TokenKind::Over
                | TokenKind::Step
                | TokenKind::Upto
                | TokenKind::Limit
                | TokenKind::Asc
                | TokenKind::Desc
                | TokenKind::Order
                | TokenKind::By
                | TokenKind::Skip
                | TokenKind::Unwind
                | TokenKind::Optional
                | TokenKind::Distinct
                | TokenKind::All
                | TokenKind::Null
                | TokenKind::Is
                | TokenKind::Not
                | TokenKind::And
                | TokenKind::Or
                | TokenKind::Xor
                | TokenKind::Contains
                | TokenKind::StartsWith
                | TokenKind::EndsWith
                | TokenKind::Case
                | TokenKind::When
                | TokenKind::Then
                | TokenKind::Else
                | TokenKind::End
                | TokenKind::Union
                | TokenKind::Intersect
                | TokenKind::Group
                | TokenKind::Between
        )
    }

    fn is_literal(&self) -> bool {
        matches!(
            self,
            TokenKind::StringLiteral(_)
                | TokenKind::IntegerLiteral(_)
                | TokenKind::FloatLiteral(_)
                | TokenKind::BooleanLiteral(_)
        )
    }

    fn is_operator(&self) -> bool {
        matches!(
            self,
            TokenKind::Plus
                | TokenKind::Minus
                | TokenKind::Star
                | TokenKind::Div
                | TokenKind::Mod
                | TokenKind::Exp
                | TokenKind::Eq
                | TokenKind::Assign
                | TokenKind::Ne
                | TokenKind::Lt
                | TokenKind::Le
                | TokenKind::Gt
                | TokenKind::Ge
                | TokenKind::Regex
                | TokenKind::NotOp
        )
    }

    fn is_delimiter(&self) -> bool {
        matches!(
            self,
            TokenKind::LParen
                | TokenKind::RParen
                | TokenKind::LBracket
                | TokenKind::RBracket
                | TokenKind::LBrace
                | TokenKind::RBrace
                | TokenKind::Comma
                | TokenKind::Dot
                | TokenKind::DotDot
                | TokenKind::Colon
                | TokenKind::Semicolon
                | TokenKind::QMark
                | TokenKind::Question
                | TokenKind::Pipe
                | TokenKind::Arrow
                | TokenKind::BackArrow
                | TokenKind::At
                | TokenKind::Dollar
        )
    }

    fn is_identifier(&self) -> bool {
        matches!(self, TokenKind::Identifier(_))
    }
}
