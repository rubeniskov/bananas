//! Plain-language descriptions of `man 5 exports` options for tooltip
//! hover-help in the UI. Keep these short — they render as native
//! HTML title attributes (single-line tooltips on most browsers).

pub fn rw() -> &'static str {
    "Read/write access. The client can both read and modify files."
}

pub fn ro() -> &'static str {
    "Read-only access. Writes from the client are rejected."
}

pub fn sync() -> &'static str {
    "Wait for writes to hit disk before replying. Safer if power dies, slower."
}

pub fn r#async() -> &'static str {
    "Reply before flushing to disk. Faster, but a server crash can lose recent writes."
}

pub fn no_subtree_check() -> &'static str {
    "Skip subtree checks. Recommended — fixes file rename races and is the modern default."
}

pub fn insecure() -> &'static str {
    "Allow client connections from non-privileged ports (>1024). Required for some clients (e.g. macOS Finder, Docker volumes)."
}

pub fn secure() -> &'static str {
    "Require connections from a privileged port (<1024). Default."
}

pub fn squash_all() -> &'static str {
    "Map every client UID/GID to anonuid/anongid. Best for shared media stores where all files should look the same on disk."
}

pub fn squash_root() -> &'static str {
    "Map only client root (UID 0) to anonuid. Default — protects the server from a hostile client root."
}

pub fn squash_no_root() -> &'static str {
    "Trust client root as server root. DANGEROUS — only use for fully-trusted clients (e.g. an admin workstation)."
}

pub fn squash_label(value: &str) -> &'static str {
    match value {
        "all_squash" => squash_all(),
        "root_squash" => squash_root(),
        "no_root_squash" => squash_no_root(),
        _ => "Unknown squash mode.",
    }
}

pub fn anonuid() -> &'static str {
    "UID stored on disk for squashed writes. 1000 = the BanaNAS service user."
}

pub fn anongid() -> &'static str {
    "GID stored on disk for squashed writes. 1000 = the BanaNAS service group."
}

pub fn squash_field() -> &'static str {
    "Identity remapping for client requests. Hover each option for details."
}

pub fn path() -> &'static str {
    "Absolute server-side path to share (e.g. /srv/media). Use the Browse button to pick one."
}

pub fn client() -> &'static str {
    "Who can mount this share: a single host (10.0.0.5), a CIDR range (10.0.0.0/24), a wildcard (*.lan), or * for any client."
}
