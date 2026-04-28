//! Parse `/etc/exports` (NFS server export table).
//!
//! Format (man 5 exports):
//!   /path        client(opt1,opt2) client2(opt3)
//!   "/path with spaces"  *(rw,sync)
//! Lines starting with `#` and blank lines are ignored.

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
}
