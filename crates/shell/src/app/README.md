# shell/src/app

The app window: `AppView` owns the open threads, which page is showing (Home or a thread), and the rail.

- `mod.rs`: state and actions: new, open and close thread; Home toggle; rail toggle; the body layout.
- `titlebar.rs`: Kit `TitleBar` holding the thread tabs, the new-thread button, and the Home, rail and
  Settings buttons. The bar is a window-drag zone, so clickable parts call `occlude()` to keep their clicks.
