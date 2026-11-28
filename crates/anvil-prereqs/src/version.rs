//! Version parsing and constraint matching for npm ranges, Python specifiers, Terraform
//! constraints and Java majors (`17`, `21+`).

use std::cmp::Ordering;

/// `major.minor.patch`; missing parts count as zero and pre-release suffixes are ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    /// Parses the leading version of `raw` (`v24.15.0`, `3.11`, `1.9.5-rc1`, `17`).
    pub fn parse(raw: &str) -> Option<Self> {
        let partial = Partial::parse(raw)?;
        (!partial.parts.is_empty()).then(|| partial.lo())
    }
}

/// A version with optional trailing parts and wildcards: `20`, `3.11`, `1.2.x`, `*`.
struct Partial {
    parts: Vec<u64>,
}

impl Partial {
    fn parse(raw: &str) -> Option<Self> {
        let text = raw.trim().trim_start_matches(['v', 'V', '=']);
        let mut parts = Vec::new();
        for segment in text.split('.').take(3) {
            if matches!(segment, "x" | "X" | "*") {
                break;
            }
            let digits: String = segment.chars().take_while(char::is_ascii_digit).collect();
            parts.push(digits.parse::<u64>().ok()?);
            if digits.len() != segment.len() {
                break;
            }
        }
        let wildcard_only = matches!(text, "*" | "x" | "X" | "");
        (!parts.is_empty() || wildcard_only).then_some(Self { parts })
    }

    fn lo(&self) -> Version {
        let at = |i: usize| self.parts.get(i).copied().unwrap_or(0);
        Version(at(0), at(1), at(2))
    }

    fn is_full(&self) -> bool {
        self.parts.len() == 3
    }

    /// The first version that no longer shares the specified parts (`1.2` -> `1.3.0`).
    fn bump(&self) -> Version {
        let Version(a, b, c) = self.lo();
        match self.parts.len() {
            0 | 1 => Version(a + 1, 0, 0),
            2 => Version(a, b + 1, 0),
            _ => Version(a, b, c + 1),
        }
    }
}

const OPERATOR_CHARS: &str = "<>=~^!";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Cmp {
    Ge(Version),
    Gt(Version),
    Lt(Version),
    Le(Version),
    Eq(Version),
    Ne(Version),
    NotIn(Version, Version),
}

impl Cmp {
    fn holds(&self, v: Version) -> bool {
        match self {
            Self::Ge(x) => v.cmp(x) != Ordering::Less,
            Self::Gt(x) => v > *x,
            Self::Lt(x) => v < *x,
            Self::Le(x) => v <= *x,
            Self::Eq(x) => v == *x,
            Self::Ne(x) => v != *x,
            Self::NotIn(lo, hi) => v < *lo || v >= *hi,
        }
    }
}

/// A parsed version constraint: alternatives (`||`) of AND-ed comparators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Constraint {
    alternatives: Vec<Vec<Cmp>>,
}

impl Constraint {
    /// Parses npm ranges, Python specifiers, Terraform constraints and Java majors.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let alternatives = raw.split("||").map(parse_all_of).collect::<Result<Vec<_>, _>>()?;
        Ok(Self { alternatives })
    }

    /// True when `version` satisfies at least one alternative.
    pub fn matches(&self, version: Version) -> bool {
        self.alternatives.iter().any(|all| all.iter().all(|c| c.holds(version)))
    }
}

/// True when `found` satisfies `constraint`; `Err` when either side cannot be parsed.
pub fn satisfies(found: &str, constraint: &str) -> Result<bool, String> {
    let version = Version::parse(found).ok_or_else(|| format!("unreadable version `{found}`"))?;
    Ok(Constraint::parse(constraint)?.matches(version))
}

fn parse_all_of(text: &str) -> Result<Vec<Cmp>, String> {
    let tokens = tokenize(text);
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if tokens.get(i + 1).is_some_and(|t| t == "-") && i + 2 < tokens.len() {
            hyphen_range(&tokens[i], &tokens[i + 2], &mut out)?;
            i += 3;
            continue;
        }
        let (op, rest) = split_operator(&tokens[i]);
        let partial =
            Partial::parse(rest).ok_or_else(|| format!("unsupported constraint `{text}`"))?;
        expand(op, &partial, &mut out).map_err(|e| format!("{e} in `{text}`"))?;
        i += 1;
    }
    Ok(out)
}

/// Splits on spaces and commas, glues a lone operator to the version after it (`>= 3.8`) and turns
/// a Java-style `21+` into `>=21`.
fn tokenize(text: &str) -> Vec<String> {
    let words = text.split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty());
    let mut out = Vec::new();
    let mut pending = String::new();
    for word in words {
        if word.chars().all(|c| OPERATOR_CHARS.contains(c)) {
            pending.push_str(word);
            continue;
        }
        let joined = format!("{pending}{word}");
        pending.clear();
        out.push(match joined.strip_suffix('+') {
            Some(base) if !base.is_empty() => format!(">={base}"),
            _ => joined,
        });
    }
    out
}

fn split_operator(token: &str) -> (&str, &str) {
    let end = token.find(|c: char| !OPERATOR_CHARS.contains(c)).unwrap_or(token.len());
    token.split_at(end)
}

fn hyphen_range(from: &str, to: &str, out: &mut Vec<Cmp>) -> Result<(), String> {
    let lo = Partial::parse(from).ok_or_else(|| format!("unsupported range start `{from}`"))?;
    let hi = Partial::parse(to).ok_or_else(|| format!("unsupported range end `{to}`"))?;
    out.push(Cmp::Ge(lo.lo()));
    out.push(if hi.is_full() { Cmp::Le(hi.lo()) } else { Cmp::Lt(hi.bump()) });
    Ok(())
}

fn expand(op: &str, p: &Partial, out: &mut Vec<Cmp>) -> Result<(), String> {
    let lo = p.lo();
    let any = p.parts.is_empty();
    match op {
        "" | "=" | "==" if any => {}
        "" | "=" | "==" if p.is_full() => out.push(Cmp::Eq(lo)),
        "" | "=" | "==" => out.extend([Cmp::Ge(lo), Cmp::Lt(p.bump())]),
        ">=" if !any => out.push(Cmp::Ge(lo)),
        ">=" => {}
        ">" if p.is_full() => out.push(Cmp::Gt(lo)),
        ">" => out.push(if any { Cmp::Lt(Version(0, 0, 0)) } else { Cmp::Ge(p.bump()) }),
        "<" => out.push(Cmp::Lt(lo)),
        "<=" if p.is_full() => out.push(Cmp::Le(lo)),
        "<=" if !any => out.push(Cmp::Lt(p.bump())),
        "<=" => {}
        "^" => out.extend([Cmp::Ge(lo), Cmp::Lt(caret_ceiling(p))]),
        "~" => out.extend([Cmp::Ge(lo), Cmp::Lt(tilde_ceiling(p))]),
        "~>" | "~=" => out.extend([Cmp::Ge(lo), Cmp::Lt(pessimistic_ceiling(p))]),
        "!=" if p.is_full() => out.push(Cmp::Ne(lo)),
        "!=" => out.push(Cmp::NotIn(lo, p.bump())),
        other => return Err(format!("unsupported operator `{other}`")),
    }
    Ok(())
}

fn caret_ceiling(p: &Partial) -> Version {
    let Version(a, b, c) = p.lo();
    if a > 0 || p.parts.len() <= 1 {
        Version(a + 1, 0, 0)
    } else if b > 0 || p.parts.len() == 2 {
        Version(0, b + 1, 0)
    } else {
        Version(0, 0, c + 1)
    }
}

fn tilde_ceiling(p: &Partial) -> Version {
    let Version(a, b, _) = p.lo();
    if p.parts.len() <= 1 { Version(a + 1, 0, 0) } else { Version(a, b + 1, 0) }
}

/// Terraform `~>` and Python `~=`: the last given part may grow, the one before may not
/// (`~> 1.9.2` is `<1.10.0`, `~=3.11` is `<4.0.0`).
fn pessimistic_ceiling(p: &Partial) -> Version {
    let Version(a, b, _) = p.lo();
    if p.is_full() { Version(a, b + 1, 0) } else { Version(a + 1, 0, 0) }
}
