# vivianne-plugins

Plugin context. A plugin gets a `Scope` from the `Context` and registers services and event handlers
through it. Dropping or disposing the scope undoes everything, newest first. See ADR 0012.

Single-threaded (`Rc`) on purpose: it lives on the UI thread. Revisit if plugins must run elsewhere.
