//! `cargo xtask deps-check`: the licence allowlist and the network denylist over the dependency graph of the shipped programs.

use crate::meta::{load, Graph};

/// Licences dependencies may use. `Ubuntu-font-1.0` is the SPDX id cargo reports for the Ubuntu font licence that the spec lists as
/// `LicenseRef-UFL-1.0`.
pub const ALLOWED: &[&str] = &[
    "MIT",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Zlib",
    "0BSD",
    "BSL-1.0",
    "CC0-1.0",
    "Unicode-3.0",
    "Unicode-DFS-2016",
    "OFL-1.1",
    "LicenseRef-UFL-1.0",
    "Ubuntu-font-1.0",
    "MPL-2.0",
];

/// Crates that open network connections or speak HTTP/TLS. None may be in a shipped program's graph.
pub const NETWORK: &[&str] = &[
    "ureq", "reqwest", "hyper", "hyper-util", "h2", "curl", "curl-sys", "isahc", "attohttpc", "surf", "ehttp", "openssl", "openssl-sys", "native-tls", "rustls",
    "rustls-pki-types", "tungstenite", "tokio-tungstenite", "webpki", "webpki-roots", "websocket", "socket2", "mio-uds",
];

#[derive(Debug, PartialEq)]
enum Tok {
    Id(String),
    Open,
    Close,
    And,
    Or,
    With,
}

fn tokens(expr: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<Tok>| {
        if !cur.is_empty() {
            out.push(match cur.as_str() {
                "AND" => Tok::And,
                "OR" => Tok::Or,
                "WITH" => Tok::With,
                _ => Tok::Id(std::mem::take(cur)),
            });
            cur.clear();
        }
    };
    for c in expr.chars() {
        match c {
            '(' => {
                flush(&mut cur, &mut out);
                out.push(Tok::Open);
            }
            ')' => {
                flush(&mut cur, &mut out);
                out.push(Tok::Close);
            }
            // the old "MIT/Apache-2.0" form means OR
            '/' => {
                flush(&mut cur, &mut out);
                out.push(Tok::Or);
            }
            c if c.is_whitespace() => flush(&mut cur, &mut out),
            c => cur.push(c),
        }
    }
    flush(&mut cur, &mut out);
    out
}

struct Parser<'a> {
    t: &'a [Tok],
    i: usize,
    allowed: &'a [&'a str],
}

impl Parser<'_> {
    // or := and ('OR' and)*      (true when any alternative passes)
    fn or(&mut self) -> bool {
        let mut ok = self.and();
        while self.t.get(self.i) == Some(&Tok::Or) {
            self.i += 1;
            let r = self.and();
            ok |= r;
        }
        ok
    }
    // and := atom ('AND' atom)*  (true when every part passes)
    fn and(&mut self) -> bool {
        let mut ok = self.atom();
        while self.t.get(self.i) == Some(&Tok::And) {
            self.i += 1;
            let r = self.atom();
            ok &= r;
        }
        ok
    }
    fn atom(&mut self) -> bool {
        match self.t.get(self.i) {
            Some(Tok::Open) => {
                self.i += 1;
                let r = self.or();
                if self.t.get(self.i) == Some(&Tok::Close) {
                    self.i += 1;
                }
                r
            }
            Some(Tok::Id(id)) => {
                self.i += 1;
                let mut full = id.clone();
                if self.t.get(self.i) == Some(&Tok::With) {
                    if let Some(Tok::Id(exc)) = self.t.get(self.i + 1) {
                        full = format!("{id} WITH {exc}");
                        self.i += 2;
                    }
                }
                self.allowed.contains(&full.as_str())
            }
            _ => {
                self.i += 1;
                false
            }
        }
    }
}

/// SPDX `OR` passes when one side passes, `AND` when all do.
pub fn passes(expr: &str, allowed: &[&str]) -> bool {
    let t = tokens(expr);
    let mut p = Parser { t: &t, i: 0, allowed };
    let ok = p.or();
    ok && p.i >= t.len()
}

/// Problems with a graph: one line per crate (empty when everything passes).
pub fn problems(g: &Graph) -> Vec<String> {
    let mut out = Vec::new();
    for id in &g.shipped {
        let p = &g.packages[id];
        if p.is_workspace {
            continue;
        }
        match &p.license {
            None => out.push(format!("{} {}: no licence expression", p.name, p.version)),
            Some(l) if !passes(l, ALLOWED) => out.push(format!("{} {}: licence {l} is not on the allowlist", p.name, p.version)),
            _ => {}
        }
        if NETWORK.contains(&p.name.as_str()) {
            out.push(format!("{} {}: a network crate in a shipped program (no network code is allowed)", p.name, p.version));
        }
    }
    out
}

pub fn run() -> Result<(), String> {
    let g = load()?;
    let bad = problems(&g);
    let crates = g.shipped.iter().filter(|id| !g.packages[*id].is_workspace).count();
    if bad.is_empty() {
        println!("deps-check: {crates} crates in the shipped programs, all licences allowed, no network crates");
        Ok(())
    } else {
        Err(format!("deps-check failed:\n  {}", bad.join("\n  ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spdx_expressions() {
        assert!(passes("MIT", ALLOWED));
        assert!(passes("MIT OR Apache-2.0", ALLOWED));
        assert!(passes("MIT/Apache-2.0", ALLOWED));
        assert!(passes("GPL-3.0 OR MIT", ALLOWED), "OR passes when one side passes");
        assert!(!passes("GPL-3.0 AND MIT", ALLOWED), "AND needs both");
        assert!(passes("(MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0", ALLOWED));
        assert!(passes("Apache-2.0 WITH LLVM-exception OR MIT", ALLOWED));
        assert!(passes("Apache-2.0 WITH LLVM-exception", ALLOWED));
        assert!(!passes("Apache-2.0 WITH Classpath-exception-2.0", ALLOWED));
        assert!(!passes("GPL-3.0-only", ALLOWED));
        assert!(!passes("", ALLOWED));
        assert!(passes("Unicode-3.0", ALLOWED) && passes("MPL-2.0", ALLOWED) && passes("Zlib OR Apache-2.0 OR MIT", ALLOWED));
    }

    #[test]
    fn network_crates_are_named() {
        assert!(NETWORK.contains(&"ureq") && NETWORK.contains(&"rustls"));
    }
}
