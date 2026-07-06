use anyhow::{Context, Result};
use std::path::PathBuf;

fn main() -> Result<()> {
    let args = parse_args()?;
    let pc = pacmanconf::Config::from_file(&args.input)
        .with_context(|| format!("parsing {}", args.input.display()))?;
    let mc = convert(&pc);
    let toml = toml::to_string_pretty(&mc).context("serializing miz.toml")?;
    match args.output.as_deref() {
        Some(path) => {
            std::fs::write(path, toml).with_context(|| format!("writing {}", path.display()))?
        }
        None => print!("{toml}"),
    }
    Ok(())
}

struct Args {
    input: PathBuf,
    output: Option<PathBuf>,
}

fn parse_args() -> Result<Args> {
    let mut args = std::env::args().skip(1);
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "-o" | "--output" => {
                output = Some(args.next().context("missing value for --output")?.into());
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("miz-convert {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            s if s.starts_with('-') => {
                anyhow::bail!("unknown flag: {s} (try --help)");
            }
            _ => {
                if input.is_some() {
                    anyhow::bail!("multiple input files; expected one");
                }
                input = Some(a.into());
            }
        }
    }
    let input = input.unwrap_or_else(|| PathBuf::from("/etc/pacman.conf"));
    Ok(Args { input, output })
}

fn print_help() {
    println!(
        "miz-convert {ver}\n\
        \n\
        Convert a pacman.conf to miz.toml. Every directive pacman-conf\n\
        recognises is preserved; deprecated `use_delta` is round-tripped\n\
        for fidelity. `Include` directives are inlined by pacman-conf\n\
        before this tool sees them — the resulting miz.toml has explicit\n\
        Server lists per repo, not Include references.\n\
        \n\
        USAGE:\n    \
        miz-convert [INPUT] [--output FILE]\n\
        \n\
        ARGS:\n    \
        INPUT    Path to pacman.conf (default: /etc/pacman.conf)\n\
        \n\
        OPTIONS:\n    \
        -o, --output <FILE>  Write to FILE instead of stdout\n    \
        -h, --help           Show this help\n    \
        -V, --version        Show version",
        ver = env!("CARGO_PKG_VERSION")
    );
}

fn convert(pc: &pacmanconf::Config) -> miz_config::MizConfig {
    miz_config::MizConfig {
        options: miz_config::Options {
            root_dir: PathBuf::from(&pc.root_dir),
            db_path: PathBuf::from(&pc.db_path),
            cache_dir: paths(&pc.cache_dir),
            hook_dir: paths(&pc.hook_dir),
            gpg_dir: PathBuf::from(&pc.gpg_dir),
            log_file: PathBuf::from(&pc.log_file),
            hold_pkg: pc.hold_pkg.clone(),
            ignore_pkg: pc.ignore_pkg.clone(),
            ignore_group: pc.ignore_group.clone(),
            architecture: pc.architecture.clone(),
            no_upgrade: pc.no_upgrade.clone(),
            no_extract: pc.no_extract.clone(),
            clean_method: pc.clean_method.clone(),
            xfer_command: optional(&pc.xfer_command),
            parallel_downloads: pc.parallel_downloads,
            disable_download_timeout: pc.disable_download_timeout,
            download_user: pc.download_user.clone(),
            sig_level: pc.sig_level.clone(),
            local_file_sig_level: pc.local_file_sig_level.clone(),
            remote_file_sig_level: pc.remote_file_sig_level.clone(),
            disable_sandbox: pc.disable_sandbox,
            disable_sandbox_filesystem: pc.disable_sandbox_filesystem,
            disable_sandbox_syscalls: pc.disable_sandbox_syscalls,
            use_syslog: pc.use_syslog,
            color: pc.color,
            total_download: pc.total_download,
            check_space: pc.check_space,
            verbose_pkg_lists: pc.verbose_pkg_lists,
            chomp: pc.chomp,
            use_delta: pc.use_delta,
        },
        repos: pc.repos.iter().map(convert_repo).collect(),
        // pacman.conf has no split-db/archive settings; a converted config is a
        // plain single-db setup.
        archetype: None,
    }
}

fn convert_repo(r: &pacmanconf::Repository) -> miz_config::Repository {
    miz_config::Repository {
        name: r.name.clone(),
        servers: r.servers.clone(),
        sig_level: r.sig_level.clone(),
        usage: r.usage.clone(),
        include: Vec::new(),
    }
}

fn paths(v: &[String]) -> Vec<PathBuf> {
    v.iter().map(PathBuf::from).collect()
}

fn optional(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn parse(src: &str) -> pacmanconf::Config {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(src.as_bytes()).unwrap();
        pacmanconf::Config::from_file(f.path()).unwrap()
    }

    #[test]
    #[ignore = "requires pacman-conf on PATH (Arch host or container)"]
    fn minimal_options_roundtrip() {
        let pc = parse(
            "[options]\n\
             Architecture = x86_64\n\
             ParallelDownloads = 7\n\
             Color\n\
             [core]\n\
             Server = https://example/$repo\n",
        );
        let mc = convert(&pc);
        assert_eq!(mc.options.architecture, vec!["x86_64".to_string()]);
        assert_eq!(mc.options.parallel_downloads, 7);
        assert!(mc.options.color);
        assert_eq!(mc.repos.len(), 1);
        assert_eq!(mc.repos[0].name, "core");
        assert_eq!(
            mc.repos[0].servers,
            vec!["https://example/core".to_string()]
        );
    }

    #[test]
    #[ignore = "requires pacman-conf on PATH (Arch host or container)"]
    fn per_repo_siglevel_and_usage() {
        let pc = parse(
            "[options]\n\
             [archive]\n\
             SigLevel = Optional TrustAll\n\
             Usage = Sync Search\n\
             Server = https://archive/$repo\n",
        );
        let mc = convert(&pc);
        assert_eq!(mc.repos[0].name, "archive");
        assert_eq!(
            mc.repos[0].sig_level,
            vec!["Optional".to_string(), "TrustAll".to_string()]
        );
        assert_eq!(
            mc.repos[0].usage,
            vec!["Sync".to_string(), "Search".to_string()]
        );
    }

    #[test]
    #[ignore = "requires pacman-conf on PATH (Arch host or container)"]
    fn full_roundtrip_serializes_to_valid_toml() {
        let pc = parse(
            "[options]\n\
             HoldPkg = pacman glibc\n\
             IgnorePkg = linux\n\
             NoExtract = etc/foo.conf\n\
             CleanMethod = KeepCurrent\n\
             XferCommand = /usr/bin/curl -L -o %o %u\n\
             DownloadUser = alpm\n\
             DisableSandbox\n\
             ILoveCandy\n\
             [extra]\n\
             Server = https://m/$repo\n",
        );
        let mc = convert(&pc);
        let serialized = toml::to_string_pretty(&mc).unwrap();
        let reparsed: miz_config::MizConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(
            reparsed.options.hold_pkg,
            vec!["pacman".to_string(), "glibc".to_string()]
        );
        assert_eq!(reparsed.options.ignore_pkg, vec!["linux".to_string()]);
        assert_eq!(
            reparsed.options.xfer_command.as_deref(),
            Some("/usr/bin/curl -L -o %o %u")
        );
        assert_eq!(reparsed.options.download_user.as_deref(), Some("alpm"));
        assert!(reparsed.options.disable_sandbox);
        assert!(reparsed.options.chomp);
    }

    #[test]
    #[ignore = "requires pacman-conf on PATH (Arch host or container)"]
    fn defaults_match_when_options_empty() {
        let pc = parse("[options]\n[core]\nServer = https://x/$repo\n");
        let mc = convert(&pc);
        // pacman-conf fills these in from /etc/pacman.conf-style defaults.
        // We don't assert exact values (host pacman-conf may vary); just
        // that the conversion didn't lose required structure.
        assert!(!mc.options.root_dir.as_os_str().is_empty());
        assert!(!mc.options.db_path.as_os_str().is_empty());
        assert_eq!(mc.repos.len(), 1);
    }
}
