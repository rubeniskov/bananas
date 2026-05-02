//! Parse / render `/etc/exports` (NFS server export table).
//!
//! Format (man 5 exports):
//!   /path        client(opt1,opt2) client2(opt3)
//!   "/path with spaces"  *(rw,sync)
//! Lines starting with `#` and blank lines are ignored.
//!
//! For the UI we operate on flat (path, host, options) rows. One row per
//! (path, client) pair — multi-client lines parse into multiple rows.

#[derive(Debug, Clone)]
pub struct Export {
    pub path: String,
    pub clients: Vec<Client>,
}

#[derive(Debug, Clone)]
pub struct Client {
    pub host: String,
    pub options: String,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub path: String,
    pub host: String,
    pub options: String,
}

pub fn parse(input: &str) -> Vec<Export> {
    input
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            parse_line(line)
        })
        .collect()
}

pub fn rows(input: &str) -> Vec<Row> {
    parse(input)
        .into_iter()
        .flat_map(|e| {
            e.clients.into_iter().map(move |c| Row {
                path: e.path.clone(),
                host: c.host,
                options: c.options,
            })
        })
        .collect()
}

/// Column-aligned `/etc/exports` output. Path is left-padded to the
/// widest path in the set so the host(options) column lines up
/// vertically — easier to scan, and `exportfs` doesn't care about
/// whitespace runs between fields.
pub fn serialize(rows: &[Row]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let formatted_paths: Vec<String> = rows
        .iter()
        .map(|r| {
            if r.path.contains(char::is_whitespace) {
                format!("\"{}\"", r.path)
            } else {
                r.path.clone()
            }
        })
        .collect();
    let path_width = formatted_paths
        .iter()
        .map(|p| p.chars().count())
        .max()
        .unwrap_or(0);

    rows.iter()
        .zip(formatted_paths.iter())
        .map(|(r, path)| {
            let pad = path_width.saturating_sub(path.chars().count());
            let spaces = " ".repeat(pad);
            if r.options.is_empty() {
                format!("{path}{spaces}  {}\n", r.host)
            } else {
                format!("{path}{spaces}  {}({})\n", r.host, r.options)
            }
        })
        .collect()
}

/// Structured view of an export's options. We keep an `extra` bucket for
/// anything we don't have a checkbox for so round-trips don't drop unknown
/// flags.
#[derive(Debug, Clone, Default)]
pub struct Opts {
    pub rw: bool,   // rw vs ro
    pub sync: bool, // sync vs async
    pub no_subtree_check: bool,
    pub squash: Squash,
    pub anonuid: Option<u32>,
    pub anongid: Option<u32>,
    pub insecure: bool,
    /// Tokens we don't know how to render as checkboxes, preserved verbatim.
    pub extra: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Squash {
    NoRootSquash,
    RootSquash,
    #[default]
    AllSquash,
}

impl Squash {
    pub fn as_str(self) -> &'static str {
        match self {
            Squash::NoRootSquash => "no_root_squash",
            Squash::RootSquash => "root_squash",
            Squash::AllSquash => "all_squash",
        }
    }

    pub fn from_form(s: &str) -> Self {
        match s {
            "no_root_squash" => Squash::NoRootSquash,
            "root_squash" => Squash::RootSquash,
            _ => Squash::AllSquash,
        }
    }
}

impl Opts {
    /// Parse a comma-joined options string ("rw,sync,no_subtree_check,...").
    pub fn parse(input: &str) -> Self {
        let mut o = Opts::default();
        // Default for ro vs rw is unset; track if we saw anything explicit.
        let mut saw_access = false;
        let mut saw_sync = false;
        for tok in input.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match tok {
                "rw" => {
                    o.rw = true;
                    saw_access = true;
                }
                "ro" => {
                    o.rw = false;
                    saw_access = true;
                }
                "sync" => {
                    o.sync = true;
                    saw_sync = true;
                }
                "async" => {
                    o.sync = false;
                    saw_sync = true;
                }
                "no_subtree_check" => o.no_subtree_check = true,
                "subtree_check" => o.no_subtree_check = false,
                "no_root_squash" => o.squash = Squash::NoRootSquash,
                "root_squash" => o.squash = Squash::RootSquash,
                "all_squash" => o.squash = Squash::AllSquash,
                "insecure" => o.insecure = true,
                "secure" => o.insecure = false,
                t if t.starts_with("anonuid=") => {
                    o.anonuid = t["anonuid=".len()..].parse().ok();
                }
                t if t.starts_with("anongid=") => {
                    o.anongid = t["anongid=".len()..].parse().ok();
                }
                other => o.extra.push(other.to_string()),
            }
        }
        // NFS man-page defaults: `ro` and `sync` if unset. Match those.
        if !saw_access {
            o.rw = false;
        }
        if !saw_sync {
            o.sync = true;
        }
        o
    }

    pub fn to_options_string(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        parts.push(if self.rw { "rw".into() } else { "ro".into() });
        parts.push(if self.sync {
            "sync".into()
        } else {
            "async".into()
        });
        if self.no_subtree_check {
            parts.push("no_subtree_check".into());
        }
        parts.push(self.squash.as_str().into());
        if let Some(uid) = self.anonuid {
            parts.push(format!("anonuid={}", uid));
        }
        if let Some(gid) = self.anongid {
            parts.push(format!("anongid={}", gid));
        }
        if self.insecure {
            parts.push("insecure".into());
        }
        for x in &self.extra {
            parts.push(x.clone());
        }
        parts.join(",")
    }
}

fn parse_line(line: &str) -> Option<Export> {
    let (path, rest) = if let Some(stripped) = line.strip_prefix('"') {
        let end = stripped.find('"')?;
        (stripped[..end].to_string(), &stripped[end + 1..])
    } else {
        let mut iter = line.splitn(2, char::is_whitespace);
        let path = iter.next()?.to_string();
        (path, iter.next().unwrap_or(""))
    };

    let clients = parse_clients(rest.trim());
    Some(Export { path, clients })
}

fn parse_clients(rest: &str) -> Vec<Client> {
    let mut out = Vec::new();
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let start = i;
        while i < bytes.len() && bytes[i] != b'(' && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let host = rest[start..i].to_string();
        let options = if i < bytes.len() && bytes[i] == b'(' {
            i += 1;
            let opt_start = i;
            while i < bytes.len() && bytes[i] != b')' {
                i += 1;
            }
            let opt = rest[opt_start..i].to_string();
            if i < bytes.len() {
                i += 1;
            }
            opt
        } else {
            String::new()
        };
        out.push(Client { host, options });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input() {
        assert!(parse("").is_empty());
    }

    #[test]
    fn comments_and_blanks_ignored() {
        let input = "# header\n\n  \n# /skipped 1.2.3.4(rw)\n";
        assert!(parse(input).is_empty());
    }

    #[test]
    fn single_client() {
        let parsed = parse("/srv/media 192.168.1.0/24(rw,sync)");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].path, "/srv/media");
        assert_eq!(parsed[0].clients.len(), 1);
        assert_eq!(parsed[0].clients[0].host, "192.168.1.0/24");
        assert_eq!(parsed[0].clients[0].options, "rw,sync");
    }

    #[test]
    fn multiple_clients() {
        let parsed = parse("/srv/services *(ro) 10.0.0.0/8(rw,sync,no_root_squash)");
        assert_eq!(parsed[0].clients.len(), 2);
        assert_eq!(parsed[0].clients[0].host, "*");
        assert_eq!(parsed[0].clients[0].options, "ro");
        assert_eq!(parsed[0].clients[1].host, "10.0.0.0/8");
        assert_eq!(parsed[0].clients[1].options, "rw,sync,no_root_squash");
    }

    #[test]
    fn quoted_path_with_spaces() {
        let parsed = parse(r#""/srv/media files" *(rw)"#);
        assert_eq!(parsed[0].path, "/srv/media files");
        assert_eq!(parsed[0].clients[0].host, "*");
    }

    #[test]
    fn host_without_options() {
        let parsed = parse("/srv/x host1 host2(rw)");
        assert_eq!(parsed[0].clients.len(), 2);
        assert_eq!(parsed[0].clients[0].host, "host1");
        assert!(parsed[0].clients[0].options.is_empty());
    }

    #[test]
    fn flatten_rows() {
        let r = rows("/a 1.1.1.1(rw) 2.2.2.2(ro)\n/b 3.3.3.3");
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].path, "/a");
        assert_eq!(r[0].host, "1.1.1.1");
        assert_eq!(r[1].host, "2.2.2.2");
        assert_eq!(r[2].path, "/b");
        assert!(r[2].options.is_empty());
    }

    #[test]
    fn round_trip_serialize() {
        let r = rows("/a 1.1.1.1(rw,sync)");
        let out = serialize(&r);
        // Two-space gutter between path column and host(options).
        assert_eq!(out, "/a  1.1.1.1(rw,sync)\n");
    }

    #[test]
    fn aligns_columns_when_paths_differ_in_length() {
        let r = rows("/short *(ro)\n/a/much/longer/path *(rw)");
        let out = serialize(&r);
        // Both rows should have the same column for the host token.
        let lines: Vec<&str> = out.lines().collect();
        let host_col_a = lines[0].find('*').unwrap();
        let host_col_b = lines[1].find('*').unwrap();
        assert_eq!(host_col_a, host_col_b);
    }

    #[test]
    fn opts_defaults() {
        let o = Opts::parse("");
        assert!(!o.rw);
        assert!(o.sync);
        assert_eq!(o.squash, Squash::AllSquash);
    }

    #[test]
    fn opts_full_round_trip() {
        let s = "rw,sync,no_subtree_check,all_squash,anonuid=1000,anongid=1000,insecure";
        let o = Opts::parse(s);
        assert!(o.rw);
        assert!(o.sync);
        assert!(o.no_subtree_check);
        assert_eq!(o.squash, Squash::AllSquash);
        assert_eq!(o.anonuid, Some(1000));
        assert_eq!(o.anongid, Some(1000));
        assert!(o.insecure);
        assert_eq!(o.to_options_string(), s);
    }

    #[test]
    fn opts_preserves_unknown() {
        let o = Opts::parse("rw,fsid=0,nohide");
        assert!(o.rw);
        assert!(o.extra.contains(&"fsid=0".to_string()));
        assert!(o.extra.contains(&"nohide".to_string()));
        let back = o.to_options_string();
        assert!(back.contains("fsid=0"));
        assert!(back.contains("nohide"));
    }
}
