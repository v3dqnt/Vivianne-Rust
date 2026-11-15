//! Plugin context: typed services, typed events, reversible registrations.
//!
//! A plugin gets a [`Scope`] and registers things through it. Everything it
//! registers is recorded as an undo step; removing the plugin runs the undo
//! steps in reverse order, so nothing leaks (ADR 0012).

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

type Handler = Rc<dyn Fn(&dyn Any)>;

#[derive(Default)]
struct Inner {
    services: HashMap<TypeId, Rc<dyn Any>>,
    handlers: HashMap<TypeId, Vec<(u64, Handler)>>,
    next_id: u64,
}

/// Shared context all plugins live in.
#[derive(Clone, Default)]
pub struct Context(Rc<RefCell<Inner>>);

impl std::fmt::Debug for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let i = self.0.borrow();
        f.debug_struct("Context")
            .field("services", &i.services.len())
            .field("event_types", &i.handlers.len())
            .finish()
    }
}

impl Context {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up a service by type.
    #[must_use]
    pub fn service<T: 'static>(&self) -> Option<Rc<T>> {
        let any = self.0.borrow().services.get(&TypeId::of::<T>())?.clone();
        any.downcast::<T>().ok()
    }

    /// Deliver an event to every current subscriber of its type.
    pub fn emit<E: 'static>(&self, event: &E) {
        // Clone the handler list first so handlers may register or remove things.
        let list: Vec<Handler> = self
            .0
            .borrow()
            .handlers
            .get(&TypeId::of::<E>())
            .map(|v| v.iter().map(|(_, h)| h.clone()).collect())
            .unwrap_or_default();
        for h in list {
            h(event);
        }
    }

    /// Start a scope for one plugin.
    #[must_use]
    pub fn scope(&self) -> Scope {
        Scope {
            ctx: self.clone(),
            undo: Vec::new(),
        }
    }
}

/// A plugin's registrations. Dropping it, or calling [`Scope::dispose`],
/// unwinds everything the plugin added.
pub struct Scope {
    ctx: Context,
    undo: Vec<Box<dyn FnOnce()>>,
}

impl std::fmt::Debug for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scope")
            .field("registrations", &self.undo.len())
            .finish_non_exhaustive()
    }
}

impl Scope {
    #[must_use]
    pub fn context(&self) -> &Context {
        &self.ctx
    }

    /// Provide a service. A later plugin providing the same type replaces it
    /// until this scope is disposed, then the previous one is restored.
    pub fn provide<T: 'static>(&mut self, service: T) {
        let key = TypeId::of::<T>();
        let prev = self
            .ctx
            .0
            .borrow_mut()
            .services
            .insert(key, Rc::new(service));
        let ctx = self.ctx.clone();
        self.undo.push(Box::new(move || {
            let mut i = ctx.0.borrow_mut();
            match prev {
                Some(p) => i.services.insert(key, p),
                None => i.services.remove(&key),
            };
        }));
    }

    /// Subscribe to events of type `E`.
    pub fn on<E: 'static>(&mut self, f: impl Fn(&E) + 'static) {
        let key = TypeId::of::<E>();
        let handler: Handler = Rc::new(move |any| {
            if let Some(e) = any.downcast_ref::<E>() {
                f(e);
            }
        });
        let id = {
            let mut i = self.ctx.0.borrow_mut();
            i.next_id += 1;
            let id = i.next_id;
            i.handlers.entry(key).or_default().push((id, handler));
            id
        };
        let ctx = self.ctx.clone();
        self.undo.push(Box::new(move || {
            if let Some(v) = ctx.0.borrow_mut().handlers.get_mut(&key) {
                v.retain(|(hid, _)| *hid != id);
            }
        }));
    }

    /// Register a custom undo step that runs on dispose.
    pub fn on_dispose(&mut self, f: impl FnOnce() + 'static) {
        self.undo.push(Box::new(f));
    }

    /// Unwind all registrations, newest first.
    pub fn dispose(&mut self) {
        while let Some(step) = self.undo.pop() {
            step();
        }
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        self.dispose();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct Ping(u32);
    struct Greeter(&'static str);

    #[test]
    fn service_is_visible_until_scope_is_disposed() {
        let ctx = Context::new();
        let mut scope = ctx.scope();
        scope.provide(Greeter("hi"));
        assert_eq!(ctx.service::<Greeter>().unwrap().0, "hi");
        scope.dispose();
        assert!(ctx.service::<Greeter>().is_none());
    }

    #[test]
    fn replaced_service_is_restored_on_dispose() {
        let ctx = Context::new();
        let mut base = ctx.scope();
        base.provide(Greeter("base"));
        {
            let mut over = ctx.scope();
            over.provide(Greeter("override"));
            assert_eq!(ctx.service::<Greeter>().unwrap().0, "override");
        }
        assert_eq!(ctx.service::<Greeter>().unwrap().0, "base");
    }

    #[test]
    fn events_reach_subscribers_and_stop_after_dispose() {
        let ctx = Context::new();
        let total = Rc::new(Cell::new(0));
        let mut scope = ctx.scope();
        let t = total.clone();
        scope.on(move |p: &Ping| t.set(t.get() + p.0));
        ctx.emit(&Ping(2));
        ctx.emit(&Ping(3));
        assert_eq!(total.get(), 5);
        drop(scope);
        ctx.emit(&Ping(10));
        assert_eq!(total.get(), 5);
    }

    #[test]
    fn undo_runs_newest_first() {
        let ctx = Context::new();
        let order = Rc::new(RefCell::new(Vec::new()));
        let mut scope = ctx.scope();
        for n in 1..=3 {
            let o = order.clone();
            scope.on_dispose(move || o.borrow_mut().push(n));
        }
        scope.dispose();
        assert_eq!(*order.borrow(), vec![3, 2, 1]);
    }
}
