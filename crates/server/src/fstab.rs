//! Parse / render `/etc/fstab` (mount table).
//!
//! Format (man 5 fstab):
//!   <device>  <mountpoint>  <fstype>  <options>  <dump>  <pass>
//! Lines starting with `#` and blank lines are ignored.
//!
//! Mirrors the structure of `exports.rs` so the UI/server flow is the
//! same: parse rows, edit one row, serialize, hand to helper.

#[derive(Debug, Clone)]
pub struct Row {
    pub source: String,
    pub mountpoint: String,
    pub fstype: String,
    pub options: String,
    pub dump: u32,
    pub pass: u32,
}

pub fn rows(input: &str) -> Vec<Row> {
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

/// Mountpoints whose entries the UI must NOT allow editing or deleting —
/// removing the row for `/` (or `/proc`, `/sys`, …) makes the next boot
/// fail. The Add form also refuses to create a NEW row aimed at one of
/// these mountpoints so an operator can't shadow a critical system mount
/// from the UI either.
const PROTECTED_MOUNTPOINTS: &[&str] = &[
    "/",
    "/boot",
    "/proc",
    "/sys",
    "/dev",
    "/dev/pts",
    "/dev/shm",
    "/run",
    "/var/run",
    "/var/volatile",
    "/var/log",
    "/tmp",
];

const PROTECTED_FSTYPES: &[&str] = &[
    "proc",
    "sysfs",
    "devpts",
    "devtmpfs",
    "cgroup",
    "cgroup2",
    "efivarfs",
    "tracefs",
    "debugfs",
    "configfs",
    "fusectl",
    "pstore",
    "mqueue",
];

/// Returns true if this row represents a system mount the UI must not
/// touch (matches mountpoint exact, mountpoint prefix /sys/* / /proc/*
/// / /dev/* / /run/*, system fstype, or `/dev/root` source).
pub fn is_protected(row: &Row) -> bool {
    if PROTECTED_MOUNTPOINTS.iter().any(|p| row.mountpoint == *p) {
        return true;
    }
    for prefix in &["/sys/", "/proc/", "/dev/", "/run/"] {
        if row.mountpoint.starts_with(prefix) {
            return true;
        }
    }
    if PROTECTED_FSTYPES.iter().any(|t| row.fstype == *t) {
        return true;
    }
    if row.source == "/dev/root" {
        return true;
    }
    false
}

/// Same predicate but applied to a candidate row before it's added.
/// Used by POST /api/fstab to refuse "shadow" entries.
pub fn is_protected_target(mountpoint: &str, fstype: &str, source: &str) -> bool {
    let row = Row {
        source: source.into(),
        mountpoint: mountpoint.into(),
        fstype: fstype.into(),
        options: String::new(),
        dump: 0,
        pass: 0,
    };
    is_protected(&row)
}

/// Column-aligned `/etc/fstab` output. Each of the first four fields
/// (source, mountpoint, fstype, options) is left-padded to the widest
/// value across the row set so the dump/pass columns line up. The
/// kernel doesn't care about whitespace runs, but humans do — and the
/// preview the UI shows is exactly what gets written to disk.
pub fn serialize(rows: &[Row]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let cells: Vec<[String; 4]> = rows
        .iter()
        .map(|r| {
            [
                quote_if_spaced(&r.source),
                quote_if_spaced(&r.mountpoint),
                r.fstype.clone(),
                if r.options.is_empty() {
                    "defaults".into()
                } else {
                    r.options.clone()
                },
            ]
        })
        .collect();

    let mut widths = [0usize; 4];
    for row in &cells {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }

    rows.iter()
        .zip(cells.iter())
        .map(|(r, row)| {
            let pad = |idx: usize| {
                let need = widths[idx].saturating_sub(row[idx].chars().count());
                " ".repeat(need)
            };
            format!(
                "{}{}  {}{}  {}{}  {}{}  {}  {}\n",
                row[0], pad(0),
                row[1], pad(1),
                row[2], pad(2),
                row[3], pad(3),
                r.dump,
                r.pass,
            )
        })
        .collect()
}

fn quote_if_spaced(s: &str) -> String {
    // fstab uses backslash-escaped octal for spaces (e.g. `\040`). Quoting
    // isn't actually a thing the kernel parses; if the path contains
    // whitespace we encode it the same way mount(8) does.
    if s.contains(char::is_whitespace) {
        s.replace(' ', r"\040").replace('\t', r"\011")
    } else {
        s.to_string()
    }
}

fn parse_line(line: &str) -> Option<Row> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 4 {
        return None;
    }
    let source = unescape(fields[0]);
    let mountpoint = unescape(fields[1]);
    let fstype = fields[2].to_string();
    let options = fields[3].to_string();
    let dump = fields.get(4).and_then(|s| s.parse().ok()).unwrap_or(0u32);
    let pass = fields.get(5).and_then(|s| s.parse().ok()).unwrap_or(0u32);
    Some(Row { source, mountpoint, fstype, options, dump, pass })
}

fn unescape(s: &str) -> String {
    s.replace(r"\040", " ").replace(r"\011", "\t")
}

/// Structured view of the option string. Common toggles get their own
/// fields; everything else round-trips through `extra` so we don't drop
/// less-common options the operator may have set.
#[derive(Debug, Clone, Default)]
pub struct Opts {
    /// `defaults` (rw, suid, dev, exec, auto, nouser, async). We keep
    /// this as an explicit toggle because it's the most common option
    /// and removing it changes a lot at once.
    pub defaults: bool,
    pub noatime: bool,
    pub nofail: bool,
    pub ro: bool,            // ro vs rw
    pub discard: bool,       // SSD trim
    pub noexec: bool,
    pub nosuid: bool,
    pub nodev: bool,
    /// systemd device-timeout in seconds (e.g. 10). None means default.
    pub device_timeout: Option<u32>,
    /// Tokens we don't have a checkbox for, preserved verbatim.
    pub extra: Vec<String>,
}

impl Opts {
    pub fn parse(input: &str) -> Self {
        let mut o = Opts::default();
        for tok in input.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match tok {
                "defaults" => o.defaults = true,
                "noatime" => o.noatime = true,
                "atime" => o.noatime = false,
                "nofail" => o.nofail = true,
                "fail" => o.nofail = false,
                "ro" => o.ro = true,
                "rw" => o.ro = false,
                "discard" => o.discard = true,
                "nodiscard" => o.discard = false,
                "noexec" => o.noexec = true,
                "exec" => o.noexec = false,
                "nosuid" => o.nosuid = true,
                "suid" => o.nosuid = false,
                "nodev" => o.nodev = true,
                "dev" => o.nodev = false,
                t if t.starts_with("x-systemd.device-timeout=") => {
                    let v = &t["x-systemd.device-timeout=".len()..];
                    // accept "10s" or "10"
                    let n = v.trim_end_matches('s').parse().ok();
                    o.device_timeout = n;
                }
                other => o.extra.push(other.to_string()),
            }
        }
        o
    }

    pub fn to_options_string(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.defaults {
            parts.push("defaults".into());
        }
        if self.noatime {
            parts.push("noatime".into());
        }
        if self.nofail {
            parts.push("nofail".into());
        }
        if self.ro {
            parts.push("ro".into());
        }
        if self.discard {
            parts.push("discard".into());
        }
        if self.noexec {
            parts.push("noexec".into());
        }
        if self.nosuid {
            parts.push("nosuid".into());
        }
        if self.nodev {
            parts.push("nodev".into());
        }
        if let Some(t) = self.device_timeout {
            parts.push(format!("x-systemd.device-timeout={t}"));
        }
        for x in &self.extra {
            parts.push(x.clone());
        }
        if parts.is_empty() {
            "defaults".into()
        } else {
            parts.join(",")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input() {
        assert!(rows("").is_empty());
    }

    #[test]
    fn comments_and_blanks_ignored() {
        let r = rows("# header\n\n  \n# /skipped foo bar baz 0 0\n");
        assert!(r.is_empty());
    }

    #[test]
    fn parses_label() {
        let r = rows("LABEL=media /srv/media ext4 defaults,noatime,nofail 0 2");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].source, "LABEL=media");
        assert_eq!(r[0].mountpoint, "/srv/media");
        assert_eq!(r[0].fstype, "ext4");
        assert_eq!(r[0].options, "defaults,noatime,nofail");
        assert_eq!(r[0].dump, 0);
        assert_eq!(r[0].pass, 2);
    }

    #[test]
    fn parses_uuid_and_path() {
        let input = "UUID=abc /mnt/x ext4 defaults 0 2\n/dev/sda1 /mnt/y vfat ro 0 0";
        let r = rows(input);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].source, "UUID=abc");
        assert_eq!(r[1].source, "/dev/sda1");
        assert_eq!(r[1].fstype, "vfat");
        assert_eq!(r[1].options, "ro");
    }

    #[test]
    fn unescapes_spaces() {
        let r = rows(r"LABEL=foo /srv/with\040space ext4 defaults 0 2");
        assert_eq!(r[0].mountpoint, "/srv/with space");
    }

    #[test]
    fn round_trip_serialize() {
        let r = vec![Row {
            source: "LABEL=media".into(),
            mountpoint: "/srv/media".into(),
            fstype: "ext4".into(),
            options: "defaults,noatime,nofail".into(),
            dump: 0,
            pass: 2,
        }];
        let s = serialize(&r);
        // The serializer uses tabs as separators; re-parse to compare values.
        let back = rows(&s);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].source, r[0].source);
        assert_eq!(back[0].options, r[0].options);
        assert_eq!(back[0].pass, 2);
    }

    #[test]
    fn empty_options_emits_defaults() {
        let r = vec![Row {
            source: "tmpfs".into(),
            mountpoint: "/tmp".into(),
            fstype: "tmpfs".into(),
            options: "".into(),
            dump: 0,
            pass: 0,
        }];
        assert!(serialize(&r).contains("defaults"));
    }

    #[test]
    fn aligns_columns_when_widths_differ() {
        let r = vec![
            Row {
                source: "LABEL=media".into(),
                mountpoint: "/srv/media".into(),
                fstype: "ext4".into(),
                options: "defaults".into(),
                dump: 0,
                pass: 2,
            },
            Row {
                source: "tmpfs".into(),
                mountpoint: "/tmp".into(),
                fstype: "tmpfs".into(),
                options: "size=512M".into(),
                dump: 0,
                pass: 0,
            },
        ];
        let out = serialize(&r);
        let lines: Vec<&str> = out.lines().collect();
        // Source column should be padded to LABEL=media's width on both rows.
        assert!(lines[0].starts_with("LABEL=media  "));
        assert!(lines[1].starts_with("tmpfs        "));
    }

    #[test]
    fn opts_round_trip() {
        let s = "defaults,noatime,nofail,x-systemd.device-timeout=10";
        let o = Opts::parse(s);
        assert!(o.defaults && o.noatime && o.nofail);
        assert_eq!(o.device_timeout, Some(10));
        assert_eq!(o.to_options_string(), s);
    }

    #[test]
    fn opts_preserves_unknown() {
        let o = Opts::parse("defaults,uquota,relatime");
        assert!(o.defaults);
        assert!(o.extra.contains(&"uquota".to_string()));
        assert!(o.extra.contains(&"relatime".to_string()));
        let back = o.to_options_string();
        assert!(back.contains("uquota"));
        assert!(back.contains("relatime"));
    }

    #[test]
    fn opts_default_when_empty() {
        let o = Opts::parse("");
        assert_eq!(o.to_options_string(), "defaults");
    }

    fn row(source: &str, mp: &str, fstype: &str) -> Row {
        Row {
            source: source.into(),
            mountpoint: mp.into(),
            fstype: fstype.into(),
            options: "defaults".into(),
            dump: 0,
            pass: 0,
        }
    }

    #[test]
    fn protected_matches_root_and_system_mounts() {
        assert!(is_protected(&row("/dev/root", "/", "ext4")));
        assert!(is_protected(&row("proc", "/proc", "proc")));
        assert!(is_protected(&row("sysfs", "/sys", "sysfs")));
        assert!(is_protected(&row("tmpfs", "/run", "tmpfs")));
        assert!(is_protected(&row("tmpfs", "/run/user/1000", "tmpfs"))); // /run/* prefix
    }

    #[test]
    fn protected_matches_fstype_even_with_unusual_mp() {
        // A row claiming fstype=proc on /weird should still be flagged.
        assert!(is_protected(&row("none", "/weird", "proc")));
    }

    #[test]
    fn user_managed_mounts_are_not_protected() {
        assert!(!is_protected(&row("LABEL=media", "/srv/media", "ext4")));
        assert!(!is_protected(&row("UUID=abc", "/mnt/x", "ext4")));
        assert!(!is_protected(&row(
            "192.168.1.5:/share",
            "/mnt/nas",
            "nfs"
        )));
    }

    #[test]
    fn protected_target_helper() {
        assert!(is_protected_target("/", "ext4", "/dev/root"));
        assert!(is_protected_target("/proc", "proc", "proc"));
        assert!(!is_protected_target("/srv/media", "ext4", "LABEL=media"));
    }
}
