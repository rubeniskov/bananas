//! `bananas-devtool` CLI — placeholder for the dev-loop subcommands
//! (`e2e`, `container`, …) outlined in
//! `/home/rubeniskov/.claude/plans/gleaming-stargazing-boot.md`.
//! For now it just prints a usage banner; per-test usage of the
//! library API is the load-bearing path until the CLI fills in.

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--version") | Some("-V") => {
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        }
        _ => {
            eprintln!(
                "bananas-devtool — workspace e2e helper (CLI subcommands not implemented yet).\n\
                 \n\
                 Use the library API from per-crate integration tests:\n\
                     use bananas_devtool::Harness;\n\
                     #[tokio::test] async fn t() {{\n\
                         let h = Harness::new().await.unwrap();\n\
                         let cookie = h.login(\"root\", bananas_devtool::TEST_PASSWORD).await.unwrap();\n\
                         // …\n\
                     }}\n\
                 \n\
                 Make sure the daemons are built first:\n\
                     cargo build --workspace --bins\n"
            );
        }
    }
}
