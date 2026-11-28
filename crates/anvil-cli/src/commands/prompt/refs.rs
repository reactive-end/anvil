//! What the lookups return: code locations, docs concepts, and memories, plus the bundle of them
//! gathered for one prompt.

use serde::Serialize;

/// A code location worth starting from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CodeRef {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: Option<u32>,
}

impl CodeRef {
    /// Stable identity used to avoid repeating the item across prompts.
    pub fn key(&self) -> String {
        format!("code:{}:{}", self.file, self.name)
    }

    /// `Order (orders-api/src/order.ts:12)`, or just the path for a file.
    pub fn render(&self) -> String {
        match self.line {
            Some(line) => format!("{} ({}:{line})", self.name, self.file),
            None => self.file.clone(),
        }
    }
}

/// A living-docs concept that matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DocRef {
    pub id: String,
    pub title: Option<String>,
    pub path: String,
    pub stale: bool,
}

impl DocRef {
    pub fn key(&self) -> String {
        format!("doc:{}", self.id)
    }

    /// `orders-api/orders — Orders module [stale]`.
    pub fn render(&self) -> String {
        let title = self.title.as_deref().map(|t| format!(" — {t}")).unwrap_or_default();
        let stale = if self.stale { " [stale]" } else { "" };
        format!("{}{title}{stale}", self.id)
    }
}

/// A memory that matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryRef {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub stale: bool,
}

impl MemoryRef {
    pub fn key(&self) -> String {
        format!("mem:{}", self.id)
    }

    /// `Use idempotency keys (decision)`.
    pub fn render(&self) -> String {
        let stale = if self.stale { ", stale" } else { "" };
        format!("{} ({}{stale})", self.title, self.kind)
    }
}

/// What was found for one prompt.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Gathered {
    pub code: Vec<CodeRef>,
    pub docs: Vec<DocRef>,
    pub memories: Vec<MemoryRef>,
    /// The prompt names a file by extension (`order.service.ts`), whether or not the graph knows it.
    pub names_a_file: bool,
    /// Gathering stopped early because the time budget ran out.
    pub out_of_time: bool,
    /// Milliseconds gathering took, graph load included.
    pub elapsed_ms: u64,
    /// Errors met while looking things up; the caller records them, they never abort the prompt.
    #[serde(skip)]
    pub problems: Vec<String>,
}

impl Gathered {
    /// `true` when the prompt points at something concrete: a resolved symbol, doc, or file.
    pub fn has_target(&self) -> bool {
        self.names_a_file || !self.code.is_empty() || !self.docs.is_empty()
    }
}
