// What a program starts with: a small console, the checks that turn a completion value into lossless JSON, and the
// installer that builds each binding namespace and its typed error class. Everything the program must not be able to
// replace is captured here, before its code runs.
(() => {
  const g = globalThis;
  const _Error = Error;
  const _TypeError = TypeError;
  const _json = JSON;
  const _stringify = JSON.stringify;
  const _parse = JSON.parse;
  const _isArray = Array.isArray;
  const _isFinite = Number.isFinite;
  const _is = Object.is;
  const _create = Object.create;
  const _defineProperty = Object.defineProperty;
  const _getPrototypeOf = Object.getPrototypeOf;
  const _getOwnPropertyDescriptor = Object.getOwnPropertyDescriptor;
  const _ownKeys = Reflect.ownKeys;
  const _hasOwn = Object.hasOwn;
  const _objectPrototype = Object.prototype;
  const _arrayPrototype = Array.prototype;
  const _String = String;
  const _invoke = g.__invoke;
  const _log = g.__log;

  const MAX_DEPTH = 1000;

  // The value as lossless JSON text, or an Error saying which part is not: undefined, functions, symbols, bigints,
  // non-finite numbers, negative zero, class instances, sparse arrays, accessors, hidden or symbol-keyed properties,
  // cycles, and anything nested deeper than MAX_DEPTH.
  const snapshot = (root, label) => {
    const active = [];
    const walk = (value, path, depth) => {
      if (depth > MAX_DEPTH) throw new _TypeError(path + " is nested too deeply");
      if (value === null) return;
      const type = typeof value;
      if (type === "boolean" || type === "string") return;
      if (type === "number") {
        if (!_isFinite(value)) throw new _TypeError(path + " is not a finite number");
        if (_is(value, -0)) throw new _TypeError(path + " is negative zero");
        return;
      }
      if (type !== "object") throw new _TypeError(path + " is " + (type === "undefined" ? "undefined" : "a " + type) + ", which is not JSON");
      for (let i = 0; i < active.length; i++) if (active[i] === value) throw new _TypeError(path + " is a circular reference");
      active[active.length] = value;
      if (_isArray(value)) {
        if (_getPrototypeOf(value) !== _arrayPrototype) throw new _TypeError(path + " is not a plain array");
        const keys = _ownKeys(value);
        if (keys.length !== value.length + 1) throw new _TypeError(path + " is a sparse array or has extra properties");
        for (let i = 0; i < value.length; i++) {
          const descriptor = _getOwnPropertyDescriptor(value, i);
          if (descriptor === undefined || !_hasOwn(descriptor, "value")) throw new _TypeError(path + "[" + i + "] is a hole or an accessor");
          walk(descriptor.value, path + "[" + i + "]", depth + 1);
        }
      } else {
        const prototype = _getPrototypeOf(value);
        if (prototype !== null && prototype !== _objectPrototype) throw new _TypeError(path + " is not a plain object");
        const keys = _ownKeys(value);
        for (let i = 0; i < keys.length; i++) {
          const key = keys[i];
          if (typeof key !== "string") throw new _TypeError(path + " has a symbol-keyed property");
          const descriptor = _getOwnPropertyDescriptor(value, key);
          if (!descriptor.enumerable) throw new _TypeError(path + "." + key + " is not enumerable");
          if (!_hasOwn(descriptor, "value")) throw new _TypeError(path + "." + key + " is an accessor");
          walk(descriptor.value, path + "." + key, depth + 1);
        }
      }
      active.length = active.length - 1;
    };
    walk(root, label, 0);
    return _stringify(root);
  };

  const describe = (error) => {
    try {
      if (error !== null && typeof error === "object" && typeof error.message === "string") {
        const head = (typeof error.name === "string" ? error.name : "Error") + ": " + error.message;
        const stack = typeof error.stack === "string" ? error.stack : "";
        return stack === "" ? head : head + "\n" + stack;
      }
      return "threw " + (typeof error === "string" ? error : _stringify(error));
    } catch (_) {
      return "threw a value that cannot be shown";
    }
  };

  const show = (value) => {
    if (typeof value === "string") return value;
    if (value === undefined) return "undefined";
    if (typeof value === "function" || typeof value === "symbol" || typeof value === "bigint") return _String(value);
    if (value instanceof _Error) return value.name + ": " + value.message;
    try {
      const text = _stringify(value);
      return text === undefined ? _String(value) : text;
    } catch (_) {
      return _String(value);
    }
  };

  const consoleObject = _create(null);
  for (const level of ["log", "info", "warn", "error", "debug"]) {
    _defineProperty(consoleObject, level, {
      enumerable: true,
      value: (...args) => {
        let text = "";
        for (let i = 0; i < args.length; i++) text += (i === 0 ? "" : " ") + show(args[i]);
        _log(text);
      },
    });
  }
  _defineProperty(g, "console", { value: consoleObject, writable: true, configurable: true });

  // The classes built for the current evaluation, by namespace index.
  const classes = [];

  // Builds each namespace as a null-prototype object whose members call the host, so a member named `__proto__` or
  // `constructor` is an ordinary own property. `spec` is JSON: [{ global, functions: [name], errorClass: {name, property} | null }].
  _defineProperty(g, "__install", {
    value: (spec) => {
      const definitions = _parse(spec);
      classes.length = 0;
      for (let i = 0; i < definitions.length; i++) {
        const definition = definitions[i];
        const namespace = _create(null);
        for (const member of definition.functions) {
          _defineProperty(namespace, member, {
            enumerable: true,
            value: (args) => _invoke(i, member, snapshot(args === undefined ? null : args, "the arguments of " + definition.global + "." + member)),
          });
        }
        _defineProperty(g, definition.global, { value: namespace, writable: true, configurable: true });
        const info = definition.errorClass;
        if (info !== null) {
          const ErrorClass = class extends _Error {
            constructor(message, member) {
              super(message);
              _defineProperty(this, "name", { value: info.name, writable: true, configurable: true });
              _defineProperty(this, info.property, { value: member, enumerable: true, writable: true, configurable: true });
            }
          };
          _defineProperty(ErrorClass, "name", { value: info.name });
          _defineProperty(g, info.name, { value: ErrorClass, writable: true, configurable: true });
          classes[i] = ErrorClass;
        }
      }
    },
  });

  // What a rejected call rejects with: the namespace's error class if it has one.
  _defineProperty(g, "__reject_with", {
    value: (index, member, message) => {
      const ErrorClass = classes[index];
      return ErrorClass === undefined ? new _Error(message) : new ErrorClass(message, member);
    },
  });
  // A REPL program's last value settles like a promise would: a promise or thenable is waited for, anything else is
  // the value itself.
  const _resolve = Promise.resolve.bind(Promise);
  _defineProperty(g, "__complete", {
    value: (value) => {
      g.__done = undefined;
      _resolve(value).then((ok) => { g.__done = { ok }; }, (err) => { g.__done = { err }; });
    },
  });
  _defineProperty(g, "__snapshot", { value: snapshot });
  _defineProperty(g, "__describe", { value: describe });
})();
