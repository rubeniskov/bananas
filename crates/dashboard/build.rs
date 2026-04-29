// Slint compiles the .slint file at build-time into Rust code that's
// pulled in via `slint::include_modules!()` from src/app.rs. The
// dashboard always has UI (it IS the UI), so no feature gate.
fn main() {
    slint_build::compile("ui/dashboard.slint").expect("compiling dashboard.slint");
}
