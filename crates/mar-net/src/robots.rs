//! `robots.txt`, for callers who run this as a service.
//!
//! Deliberately small. The file is advisory and its grammar is loose, so the
//! useful behaviour is to read the groups that apply to us, honour the longest
//! matching rule as the standard prescribes, and treat anything unparseable as
//! permission rather than as prohibition.

/// The `Allow` and `Disallow` paths that apply to one user agent on one host.
#[derive(Debug, Clone, Default)]
pub struct Rules {
    /// `(path, allowed)`, in the order they were read.
    rules: Vec<(String, bool)>,
}

impl Rules {
    /// Parse the groups matching `ua`, falling back to the `*` group.
    ///
    /// A group naming us specifically wins outright: a site that allows
    /// everyone and then singles us out means the second thing.
    pub fn parse(body: &str, ua: &str) -> Self {
        let ua = ua.to_ascii_lowercase();
        let mut specific: Vec<(String, bool)> = Vec::new();
        let mut wildcard: Vec<(String, bool)> = Vec::new();

        // A run of `User-agent` lines shares the one group that follows them.
        let mut agents: Vec<String> = Vec::new();
        let mut in_group = false;

        for line in body.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((field, value)) = line.split_once(':') else {
                continue;
            };
            let field = field.trim().to_ascii_lowercase();
            let value = value.trim();

            match field.as_str() {
                "user-agent" => {
                    if in_group {
                        agents.clear();
                        in_group = false;
                    }
                    agents.push(value.to_ascii_lowercase());
                }
                "allow" | "disallow" => {
                    in_group = true;
                    if value.is_empty() && field == "disallow" {
                        // "Disallow:" with nothing after it allows everything,
                        // which is the absence of a rule.
                        continue;
                    }
                    let allowed = field == "allow";
                    for agent in &agents {
                        if agent == "*" {
                            wildcard.push((value.to_owned(), allowed));
                        } else if ua.contains(agent.as_str()) {
                            specific.push((value.to_owned(), allowed));
                        }
                    }
                }
                _ => {}
            }
        }

        Rules {
            rules: if specific.is_empty() {
                wildcard
            } else {
                specific
            },
        }
    }

    /// May we fetch this path?
    ///
    /// The longest matching pattern decides, and `Allow` wins a tie, which is
    /// what the standard says and what every crawler implements.
    pub fn allows(&self, path: &str) -> bool {
        let mut best: Option<(usize, bool)> = None;
        for (pattern, allowed) in &self.rules {
            if !path_matches(pattern, path) {
                continue;
            }
            let len = pattern.len();
            match best {
                Some((best_len, _)) if best_len > len => {}
                Some((best_len, _)) if best_len == len && *allowed => best = Some((len, true)),
                Some((best_len, _)) if best_len == len => {}
                _ => best = Some((len, *allowed)),
            }
        }
        best.map(|(_, allowed)| allowed).unwrap_or(true)
    }
}

/// `*` matches any run of characters, and a trailing `$` anchors the end.
fn path_matches(pattern: &str, path: &str) -> bool {
    let (pattern, anchored) = match pattern.strip_suffix('$') {
        Some(rest) => (rest, true),
        None => (pattern, false),
    };

    let mut segments = pattern.split('*');
    let Some(first) = segments.next() else {
        return true;
    };
    let Some(mut rest) = path.strip_prefix(first) else {
        return false;
    };

    let mut last_is_wildcard = false;
    for segment in segments {
        last_is_wildcard = segment.is_empty();
        if segment.is_empty() {
            continue;
        }
        let Some(at) = rest.find(segment) else {
            return false;
        };
        rest = &rest[at + segment.len()..];
        last_is_wildcard = false;
    }

    !anchored || rest.is_empty() || last_is_wildcard
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "\
User-agent: *
Disallow: /private
Allow: /private/public

User-agent: mar
Disallow: /nope
";

    #[test]
    fn a_group_naming_us_replaces_the_wildcard_group() {
        let rules = Rules::parse(BODY, "Mozilla/5.0 mar/1.0");
        assert!(rules.allows("/private"), "the wildcard group no longer applies");
        assert!(!rules.allows("/nope"));
    }

    #[test]
    fn the_wildcard_group_is_used_when_nothing_names_us() {
        let rules = Rules::parse(BODY, "Mozilla/5.0 Chrome/140");
        assert!(!rules.allows("/private"));
        assert!(rules.allows("/nope"));
    }

    #[test]
    fn the_longest_match_decides() {
        let rules = Rules::parse(BODY, "Chrome");
        assert!(
            rules.allows("/private/public/page"),
            "a longer Allow beats a shorter Disallow"
        );
    }

    #[test]
    fn wildcards_and_anchors_are_understood() {
        let rules = Rules::parse("User-agent: *\nDisallow: /*.pdf$\n", "any");
        assert!(!rules.allows("/docs/report.pdf"));
        assert!(rules.allows("/docs/report.pdf.html"));
    }

    #[test]
    fn an_empty_or_broken_file_permits_everything() {
        assert!(Rules::parse("", "any").allows("/anything"));
        assert!(Rules::parse("<!DOCTYPE html><h1>404", "any").allows("/anything"));
        assert!(
            Rules::parse("User-agent: *\nDisallow:\n", "any").allows("/anything"),
            "a bare Disallow is the absence of a rule"
        );
    }
}
