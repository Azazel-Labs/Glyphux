//! msdfgen-style argument compatibility.
//!
//! msdfgen and msdf-atlas-gen use single-dash long options (`-font`, `-pxrange`). The tools
//! here are defined with clap's standard `--long` options; before parsing, single-dash words
//! that name a known long option (or alias) are rewritten to the double-dash form, so both
//! `-pxrange 4` and `--pxrange 4` work. Negative numbers and real short flags are untouched.

use std::collections::HashSet;
use std::ffi::OsString;

/// Rewrites `-name` / `-name=value` to `--name` / `--name=value` for every long option name
/// (including aliases) of `cmd`. Arguments after a bare `--` are left alone.
pub fn normalize_args(cmd: &clap::Command, args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut longs: HashSet<String> = HashSet::new();
    for a in cmd.get_arguments() {
        if let Some(l) = a.get_long() {
            longs.insert(l.to_string());
        }
        if let Some(aliases) = a.get_all_aliases() {
            for al in aliases {
                longs.insert(al.to_string());
            }
        }
    }
    longs.insert("help".into());
    longs.insert("version".into());
    let mut out = Vec::new();
    let mut passthrough = false;
    for (i, arg) in args.into_iter().enumerate() {
        if i == 0 || passthrough {
            out.push(arg);
            continue;
        }
        let Some(s) = arg.to_str() else {
            out.push(arg);
            continue;
        };
        if s == "--" {
            passthrough = true;
            out.push(arg);
            continue;
        }
        if s.len() > 2 && s.starts_with('-') && !s.starts_with("--") {
            let body = &s[1..];
            let name = body.split('=').next().unwrap_or(body);
            if longs.contains(name) {
                out.push(OsString::from(format!("-{s}")));
                continue;
            }
        }
        out.push(arg);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Arg, Command};

    #[test]
    fn rewrites_known_single_dash_longs() {
        let cmd = Command::new("t")
            .arg(Arg::new("pxrange").long("pxrange"))
            .arg(Arg::new("size").long("dimensions").alias("size"))
            .arg(Arg::new("o").short('o'));
        let args = [
            "t", "-pxrange", "4", "-size=8", "-o", "x", "-1.5", "-unknown", "--", "-pxrange",
        ]
        .map(OsString::from);
        let out: Vec<String> = normalize_args(&cmd, args)
            .into_iter()
            .map(|s| s.into_string().unwrap())
            .collect();
        assert_eq!(
            out,
            [
                "t",
                "--pxrange",
                "4",
                "--size=8",
                "-o",
                "x",
                "-1.5",
                "-unknown",
                "--",
                "-pxrange"
            ]
        );
    }
}
