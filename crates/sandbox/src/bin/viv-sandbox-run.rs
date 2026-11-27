//! The Windows runner as a program of its own, for the tests.

fn main() {
    vivianne_sandbox::runner::run_if_asked();
    eprintln!("viv-sandbox-run: expected {} as the first argument", vivianne_sandbox::runner::ARGUMENT);
    std::process::exit(vivianne_sandbox::runner::FAILURE_EXIT);
}
