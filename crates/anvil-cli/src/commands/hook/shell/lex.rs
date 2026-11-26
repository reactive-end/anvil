//! Quote-aware lexer splitting POSIX/PowerShell command text into segments and nested scripts.

/// One simple command between shell control operators.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Segment {
    pub args: Vec<String>,
    /// Files redirected into the command with `<`.
    pub inputs: Vec<String>,
    /// Targets of `>` and `>>` redirections.
    pub outputs: Vec<String>,
    /// Here-document bodies and here-string words fed to stdin.
    pub stdin_text: Vec<String>,
    /// Whether the command reads the previous segment through a pipe.
    pub piped_in: bool,
    /// Whether stdout is consumed by a pipe or a file instead of reaching the terminal.
    pub captured_out: bool,
}

/// Parsed command text: top-level segments plus the text of every command substitution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Script {
    pub segments: Vec<Segment>,
    pub nested: Vec<String>,
}

/// Parses `cmd` without executing or expanding anything.
pub fn parse(cmd: &str) -> Script {
    let mut lexer = Lexer { chars: cmd.chars().collect(), ..Lexer::default() };
    lexer.run();
    lexer.out
}

#[derive(Debug)]
struct Heredoc {
    delimiter: String,
    expands: bool,
    segment: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Redirect {
    Input,
    HereString,
    Output,
}

#[derive(Debug, Default)]
struct Lexer {
    chars: Vec<char>,
    pos: usize,
    word: String,
    in_word: bool,
    redirect: Option<Redirect>,
    seg: Segment,
    heredocs: Vec<Heredoc>,
    ambiguous_escape: Option<usize>,
    out: Script,
}

impl Lexer {
    fn run(&mut self) {
        while let Some(ch) = self.peek(0) {
            match ch {
                '\'' | '"' => self.read_quoted(ch),
                '\n' | '\r' => self.newline(),
                c if c.is_whitespace() => {
                    self.end_word();
                    self.pos += 1;
                }
                '\\' | '`' => self.read_escape(ch),
                '$' if self.peek(1) == Some('(') => self.read_substitution(),
                '<' | '>' => self.read_redirect(ch),
                '#' if !self.in_word => self.skip_comment(),
                _ => self.read_operator_or_char(ch),
            }
        }
        self.end_segment(false);
    }

    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.pos + ahead).copied()
    }

    fn push(&mut self, ch: char) {
        self.word.push(ch);
        self.in_word = true;
    }

    fn end_word(&mut self) {
        if !self.in_word {
            return;
        }
        let word = std::mem::take(&mut self.word);
        self.in_word = false;
        match self.redirect.take() {
            Some(Redirect::Input) => self.seg.inputs.push(word),
            Some(Redirect::HereString) => self.seg.stdin_text.push(word),
            Some(Redirect::Output) => self.seg.outputs.push(word),
            None => self.seg.args.push(word),
        }
    }

    fn end_segment(&mut self, pipes_into_next: bool) {
        self.end_word();
        self.redirect = None;
        let empty = self.seg.args.is_empty()
            && self.seg.inputs.is_empty()
            && self.seg.outputs.is_empty()
            && self.seg.stdin_text.is_empty();
        if empty {
            self.seg.piped_in |= pipes_into_next;
            return;
        }
        self.seg.captured_out |= pipes_into_next;
        let next = Segment { piped_in: pipes_into_next, ..Segment::default() };
        self.out.segments.push(std::mem::replace(&mut self.seg, next));
    }

    fn newline(&mut self) {
        self.end_segment(false);
        self.pos += 1;
        if self.heredocs.is_empty() {
            return;
        }
        if self.peek(0) == Some('\n') && self.chars[self.pos - 1] == '\r' {
            self.pos += 1;
        }
        for doc in std::mem::take(&mut self.heredocs) {
            let body = self.read_heredoc_body(&doc.delimiter);
            if doc.expands {
                self.out.nested.extend(substitutions_in(&body));
            }
            if let Some(seg) = self.out.segments.get_mut(doc.segment) {
                seg.stdin_text.push(body);
            }
        }
    }

    fn read_heredoc_body(&mut self, delimiter: &str) -> String {
        let mut body = String::new();
        while self.pos < self.chars.len() {
            let start = self.pos;
            while self.peek(0).is_some_and(|c| c != '\n') {
                self.pos += 1;
            }
            let line: String = self.chars[start..self.pos].iter().collect();
            self.pos = (self.pos + 1).min(self.chars.len());
            if line.trim() == delimiter {
                break;
            }
            body.push_str(&line);
            body.push('\n');
        }
        body
    }

    fn skip_comment(&mut self) {
        while self.peek(0).is_some_and(|c| c != '\n') {
            self.pos += 1;
        }
    }

    fn read_escape(&mut self, ch: char) {
        let next = self.peek(1);
        if next == Some('\n') {
            self.pos += 2;
        } else if next == Some('\r') && self.peek(2) == Some('\n') {
            self.pos += 3;
        } else if ch == '`' {
            self.read_backtick();
        } else if let Some(escaped) = next.filter(|c| matches!(c, '"' | '\'' | '$' | '`')) {
            self.push(escaped);
            self.pos += 2;
        } else {
            self.push(ch);
            self.pos += 1;
        }
    }

    fn read_backtick(&mut self) {
        let rest = &self.chars[self.pos + 1..];
        if let Some(len) = rest.iter().position(|c| *c == '`') {
            self.out.nested.push(rest[..len].iter().collect());
            self.pos += len + 2;
            self.in_word = true;
        } else {
            self.push('`');
            self.pos += 1;
        }
    }

    /// Consumes `$(...)`, `<(...)`, or `>(...)` starting at the two-character opener.
    fn read_substitution(&mut self) {
        let start = self.pos + 2;
        let end = matching_paren(&self.chars, start);
        self.out.nested.push(self.chars[start..end].iter().collect());
        self.pos = (end + 1).min(self.chars.len());
        self.in_word = true;
    }

    fn read_quoted(&mut self, quote: char) {
        if self.read_here_string_literal(quote) {
            return;
        }
        self.in_word = true;
        self.pos += 1;
        while let Some(ch) = self.peek(0) {
            if quote == '"' && self.read_in_double_quotes(ch) {
                continue;
            }
            self.pos += 1;
            if ch == quote {
                return;
            }
            self.word.push(ch);
        }
        if let Some(at) = self.ambiguous_escape.take() {
            self.out.nested.push(self.chars[at..].iter().collect());
        }
    }

    /// Consumes escapes and substitutions inside double quotes, returning `true` when handled.
    ///
    /// `\"` closes the string in PowerShell/cmd but escapes the quote in POSIX shells, so the
    /// position is remembered and re-parsed if the POSIX reading leaves the string unterminated.
    fn read_in_double_quotes(&mut self, ch: char) -> bool {
        match (ch, self.peek(1)) {
            ('\\', Some('"')) => {
                if !self.chars[self.pos + 2..].contains(&'"') {
                    return false;
                }
                self.ambiguous_escape.get_or_insert(self.pos + 2);
                self.word.push('"');
                self.pos += 2;
                true
            }
            ('\\', Some(next @ ('$' | '`'))) => {
                self.word.push(next);
                self.pos += 2;
                true
            }
            ('$', Some('(')) => {
                self.read_substitution();
                true
            }
            ('`', _) => {
                self.read_backtick();
                true
            }
            _ => false,
        }
    }

    /// Consumes a PowerShell here-string (`@'` ... `'@`) as one literal word.
    fn read_here_string_literal(&mut self, quote: char) -> bool {
        if !self.word.ends_with('@') || !matches!(self.peek(1), Some('\n' | '\r')) {
            return false;
        }
        let closing = ['\n', quote, '@'];
        let body = &self.chars[self.pos + 1..];
        let Some(len) = body.windows(3).position(|w| w == closing) else {
            return false;
        };
        let text: String = body[..len].iter().collect();
        self.word.pop();
        self.word.push_str(&text);
        self.in_word = true;
        self.pos += 1 + len + closing.len();
        true
    }

    fn read_redirect(&mut self, ch: char) {
        if self.peek(1) == Some('(') {
            self.read_substitution();
            return;
        }
        let descriptor = self.take_descriptor();
        self.end_word();
        if ch == '>' {
            self.pos += 1;
            while matches!(self.peek(0), Some('>' | '&' | '|')) {
                self.pos += 1;
            }
            self.seg.captured_out |= descriptor.as_deref().is_none_or(|fd| fd == "1");
            self.redirect = Some(Redirect::Output);
        } else if self.peek(1) == Some('<') && self.peek(2) == Some('<') {
            self.pos += 3;
            self.redirect = Some(Redirect::HereString);
        } else if self.peek(1) == Some('<') {
            self.read_heredoc_start();
        } else {
            self.pos += 1;
            self.redirect = Some(Redirect::Input);
        }
    }

    /// Drops a numeric file descriptor glued to the redirect operator (`2>`) from the arguments.
    fn take_descriptor(&mut self) -> Option<String> {
        if self.word.is_empty() || !self.word.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        self.in_word = false;
        Some(std::mem::take(&mut self.word))
    }

    fn read_heredoc_start(&mut self) {
        self.pos += 2;
        if self.peek(0) == Some('-') {
            self.pos += 1;
        }
        while matches!(self.peek(0), Some(' ' | '\t')) {
            self.pos += 1;
        }
        let mut delimiter = String::new();
        let mut expands = true;
        while let Some(c) = self.peek(0).filter(|c| !c.is_whitespace() && !";|&<>()".contains(*c)) {
            if matches!(c, '\'' | '"' | '\\') {
                expands = false;
            } else {
                delimiter.push(c);
            }
            self.pos += 1;
        }
        if !delimiter.is_empty() {
            let segment = self.out.segments.len();
            self.heredocs.push(Heredoc { delimiter, expands, segment });
        }
    }

    fn read_operator_or_char(&mut self, ch: char) {
        let next = self.peek(1);
        match ch {
            '&' if next == Some('>') => {
                self.end_word();
                self.pos += 1;
            }
            '|' => {
                let is_or = next == Some('|');
                self.pos += if is_or || next == Some('&') { 2 } else { 1 };
                self.end_segment(!is_or);
            }
            '&' | ';' | '(' | ')' => {
                self.pos += if ch == '&' && next == Some('&') { 2 } else { 1 };
                self.end_segment(false);
            }
            '{' if self.word == "%" || next.is_none_or(char::is_whitespace) => {
                self.pos += 1;
                self.end_segment(false);
            }
            '}' if !self.in_word => {
                self.pos += 1;
                self.end_segment(false);
            }
            _ => {
                self.push(ch);
                self.pos += 1;
            }
        }
    }
}

fn matching_paren(chars: &[char], start: usize) -> usize {
    let mut depth = 1usize;
    let mut quote: Option<char> = None;
    for (idx, ch) in chars.iter().enumerate().skip(start) {
        match (quote, *ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, c @ ('"' | '\'')) => quote = Some(c),
            (None, '(') => depth += 1,
            (None, ')') => {
                depth -= 1;
                if depth == 0 {
                    return idx;
                }
            }
            (None, _) => {}
        }
    }
    chars.len()
}

fn substitutions_in(body: &str) -> Vec<String> {
    let chars: Vec<char> = body.chars().collect();
    let mut out = Vec::new();
    let mut idx = 0usize;
    while idx < chars.len() {
        if chars[idx] == '\\' {
            idx += 2;
        } else if chars[idx] == '$' && chars.get(idx + 1) == Some(&'(') {
            let end = matching_paren(&chars, idx + 2);
            out.push(chars[idx + 2..end].iter().collect());
            idx = end + 1;
        } else if chars[idx] == '`' {
            let Some(len) = chars[idx + 1..].iter().position(|c| *c == '`') else {
                break;
            };
            out.push(chars[idx + 1..idx + 1 + len].iter().collect());
            idx += len + 2;
        } else {
            idx += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::parse;

    fn args(cmd: &str) -> Vec<Vec<String>> {
        parse(cmd).segments.into_iter().map(|s| s.args).collect()
    }

    #[test]
    fn splits_operators_newlines_and_redirects() {
        assert_eq!(
            args("echo a\nrm -rf / 2>&1 >/dev/null & ls"),
            vec![vec!["echo", "a"], vec!["rm", "-rf", "/"], vec!["ls"]]
        );
        assert_eq!(args("cmd &> out.log; x >| y"), vec![vec!["cmd"], vec!["x"]]);
        let piped = parse("cat < .env | bash");
        assert_eq!(piped.segments[0].inputs, vec![".env"]);
        assert!(piped.segments[1].piped_in && !piped.segments[0].piped_in);
        assert!(piped.segments[0].captured_out && !piped.segments[1].captured_out);
        assert!(!parse("cat a 2>/dev/null").segments[0].captured_out);
        assert_eq!(args("rm -rf {a,b} ${HOME}/x"), vec![vec!["rm", "-rf", "{a,b}", "${HOME}/x"]]);
    }

    #[test]
    fn keeps_heredoc_and_here_string_bodies_out_of_commands() {
        let quoted = parse("git commit -F - <<'EOF'\ndon't `git reset --hard`\nEOF\nls");
        assert_eq!(quoted.segments.len(), 2);
        assert!(quoted.nested.is_empty());
        assert_eq!(parse("cat <<EOF\n$(git clean -f)\nEOF").nested, vec!["git clean -f"]);
        let ps = args("git commit -m @'\nit's rm -rf /\n'@");
        assert_eq!(ps, vec![vec!["git", "commit", "-m", "\nit's rm -rf /"]]);
    }

    #[test]
    fn collects_nested_substitutions() {
        assert_eq!(parse("echo \"$(git reset --hard)\"").nested, vec!["git reset --hard"]);
        assert_eq!(parse("echo `rm -rf /`").nested, vec!["rm -rf /"]);
        assert_eq!(parse("cat <(rm -rf ~)").nested, vec!["rm -rf ~"]);
        assert!(parse("echo '$(rm -rf /)'").nested.is_empty());
    }
}
