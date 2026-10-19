//! Token estimation and strict budget enforcement for tool outputs.

/// Approximate token count (≈ 4 characters per token for code-like text, rounding up).
pub fn estimate_tokens(text: &str) -> usize {
    let chars = text.chars().count();
    chars.div_ceil(4)
}

/// Accumulates result lines while respecting an optional token budget.
///
/// The truncation marker is always accounted for, so the rendered output never
/// exceeds the budget.
pub struct BudgetWriter {
    budget: Option<usize>,
    header: Vec<String>,
    lines: Vec<String>,
    used: usize,
    shown: usize,
    total: usize,
    truncated: bool,
}

const MARKER_RESERVE: usize = 32;

impl BudgetWriter {
    pub fn new(budget: Option<usize>) -> Self {
        BudgetWriter {
            budget: budget.filter(|b| *b > 0),
            header: Vec::new(),
            lines: Vec::new(),
            used: 0,
            shown: 0,
            total: 0,
            truncated: false,
        }
    }

    /// Header lines are always emitted (counted against the budget first).
    pub fn header(&mut self, line: impl Into<String>) {
        let line = line.into();
        self.used += estimate_tokens(&line) + 1;
        self.header.push(line);
    }

    /// Adds one result item (possibly multi-line). Returns false if it did not fit.
    pub fn push(&mut self, item: impl Into<String>) -> bool {
        self.total += 1;
        if self.truncated {
            return false;
        }
        let item = item.into();
        let cost = estimate_tokens(&item) + 1;
        if let Some(b) = self.budget {
            if self.used + cost + MARKER_RESERVE > b {
                self.truncated = true;
                return false;
            }
        }
        self.used += cost;
        self.shown += 1;
        self.lines.push(item);
        true
    }

    /// Records items that were not even rendered (e.g. beyond a SQL limit).
    pub fn add_unshown(&mut self, n: usize) {
        if n > 0 {
            self.total += n;
            self.truncated = true;
        }
    }

    pub fn is_full(&self) -> bool {
        self.truncated
    }

    pub fn shown(&self) -> usize {
        self.shown
    }

    pub fn finish(self) -> String {
        let mut out = String::new();
        for h in &self.header {
            out.push_str(h);
            out.push('\n');
        }
        for l in &self.lines {
            out.push_str(l);
            out.push('\n');
        }
        if self.truncated {
            match self.budget {
                Some(b) => out.push_str(&format!(
                    "[TRUNCATED: Showing {} of {} results to fit {} token budget ({} tokens used)]",
                    self.shown, self.total, b, self.used
                )),
                None => out.push_str(&format!("[TRUNCATED: Showing {} of {} results]", self.shown, self.total)),
            }
            out.push('\n');
        }
        if out.ends_with('\n') {
            out.pop();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_is_respected_and_marked() {
        let mut w = BudgetWriter::new(Some(100));
        w.header("# results");
        for i in 0..50 {
            w.push(format!("src/some/long/path/file_{i}.ts:10-20 function doSomething{i}()"));
        }
        let out = w.finish();
        assert!(estimate_tokens(&out) <= 100, "{} tokens", estimate_tokens(&out));
        assert!(out.contains("[TRUNCATED: Showing"));
        assert!(out.contains("of 50 results to fit 100 token budget"));
    }

    #[test]
    fn no_budget_no_truncation() {
        let mut w = BudgetWriter::new(None);
        for i in 0..10 {
            w.push(format!("line {i}"));
        }
        assert!(!w.finish().contains("TRUNCATED"));
    }
}
