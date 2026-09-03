// Browser environment built on top of the small native `__mar` surface.
//
// Everything here is plain JavaScript on purpose: it is the layer most likely
// to need changing as real pages hit gaps, and changing JS does not mean
// touching Rust bindings or rebuilding the engine's unsafe surface.
'use strict';

(function () {
  const native = __mar;
  delete globalThis.__mar;

  const NodeProto = Node.prototype;
  const define = (obj, name, desc) =>
    Object.defineProperty(obj, name, { configurable: true, ...desc });
  const method = (obj, name, fn) =>
    define(obj, name, { writable: true, value: fn });

  // -- console ------------------------------------------------------------

  const format = (args) =>
    args
      .map((a) => {
        if (typeof a === 'string') return a;
        // QuickJS puts only frames in .stack, unlike V8 which prefixes the
        // message. Build the full text so a logged error is readable either way.
        if (a instanceof Error) {
          const head = `${a.name || 'Error'}: ${a.message}`;
          const stack = a.stack ? String(a.stack).trimEnd() : '';
          return stack && !stack.startsWith(head) ? `${head}\n${stack}` : stack || head;
        }
        try {
          return JSON.stringify(a, jsonSafe) ?? String(a);
        } catch {
          return String(a);
        }
      })
      .join(' ');

  // Cycles and DOM nodes are common in logged objects and must not throw.
  const seen = new WeakSet();
  function jsonSafe(key, value) {
    if (value instanceof Node) return `[${value.nodeName}]`;
    if (typeof value === 'function') return `[Function ${value.name || 'anonymous'}]`;
    if (typeof value === 'bigint') return value.toString();
    if (value && typeof value === 'object') {
      if (seen.has(value)) return '[Circular]';
      seen.add(value);
    }
    return value;
  }

  const console = {
    log: (...a) => native.log_log(format(a)),
    info: (...a) => native.log_info(format(a)),
    warn: (...a) => native.log_warn(format(a)),
    error: (...a) => native.log_error(format(a)),
    debug: (...a) => native.log_debug(format(a)),
    trace: (...a) => native.log_debug(format(a)),
    dir: (...a) => native.log_log(format(a)),
    table: (...a) => native.log_log(format(a)),
    group: (...a) => native.log_log(format(a)),
    groupCollapsed: (...a) => native.log_log(format(a)),
    groupEnd: () => {},
    assert: (cond, ...a) => {
      if (!cond) native.log_error('Assertion failed: ' + format(a));
    },
    count: () => {},
    time: () => {},
    timeEnd: () => {},
  };
  globalThis.console = console;

  // -- timers -------------------------------------------------------------

  // Extra arguments to setTimeout are passed on to the callback, which some
  // libraries rely on.
  const wrapArgs = (fn, args) =>
    args.length ? () => fn(...args) : fn;
  const asFn = (fn) =>
    typeof fn === 'function' ? fn : () => globalThis.eval(String(fn));

  globalThis.setTimeout = (fn, delay, ...args) =>
    native.set_timeout(wrapArgs(asFn(fn), args), delay);
  globalThis.setInterval = (fn, delay, ...args) =>
    native.set_interval(wrapArgs(asFn(fn), args), delay);
  globalThis.clearTimeout = (id) => native.clear_timer(id);
  globalThis.clearInterval = (id) => native.clear_timer(id);
  globalThis.requestAnimationFrame = (fn) =>
    native.request_animation_frame(() => fn(native.now()));
  globalThis.cancelAnimationFrame = (id) => native.clear_timer(id);
  globalThis.requestIdleCallback = (fn) =>
    native.request_idle_callback(() =>
      fn({ didTimeout: false, timeRemaining: () => 0 })
    );
  globalThis.cancelIdleCallback = (id) => native.clear_timer(id);
  globalThis.queueMicrotask = (fn) => Promise.resolve().then(fn);
  globalThis.setImmediate = (fn, ...args) => native.set_timeout(wrapArgs(asFn(fn), args), 0);

  // -- events -------------------------------------------------------------

  class Event {
    constructor(type, init = {}) {
      this.type = String(type);
      this.bubbles = !!init.bubbles;
      this.cancelable = !!init.cancelable;
      this.composed = !!init.composed;
      this.defaultPrevented = false;
      this.target = null;
      this.currentTarget = null;
      this.eventPhase = 0;
      this.timeStamp = native.now();
      // Events we synthesise come from the engine, not from a script.
      this.isTrusted = true;
      this.cancelBubble = false;
      this._stopped = false;
      this._stoppedImmediate = false;
    }
    preventDefault() {
      if (this.cancelable) this.defaultPrevented = true;
    }
    stopPropagation() {
      this._stopped = true;
    }
    stopImmediatePropagation() {
      this._stopped = true;
      this._stoppedImmediate = true;
    }
    composedPath() {
      const path = [];
      let n = this.target;
      while (n) {
        path.push(n);
        n = n.parentNode;
      }
      return path;
    }
  }

  class CustomEvent extends Event {
    constructor(type, init = {}) {
      super(type, init);
      this.detail = init.detail ?? null;
    }
  }

  class MessageEvent extends Event {
    constructor(type, init = {}) {
      super(type, init);
      this.data = init.data ?? null;
      this.origin = init.origin ?? '';
    }
  }

  class UIEvent extends Event {
    constructor(type, init = {}) {
      super(type, init);
      this.detail = init.detail ?? 0;
      this.view = globalThis;
    }
  }

  const modifierFlags = (event, init) => {
    event.ctrlKey = !!init.ctrlKey;
    event.shiftKey = !!init.shiftKey;
    event.altKey = !!init.altKey;
    event.metaKey = !!init.metaKey;
    event.getModifierState = (k) => !!event[k.toLowerCase() + 'Key'];
  };

  class MouseEvent extends UIEvent {
    constructor(type, init = {}) {
      super(type, init);
      this.screenX = init.screenX ?? 0;
      this.screenY = init.screenY ?? 0;
      this.clientX = init.clientX ?? 0;
      this.clientY = init.clientY ?? 0;
      this.pageX = this.clientX;
      this.pageY = this.clientY;
      this.x = this.clientX;
      this.y = this.clientY;
      this.button = init.button ?? 0;
      this.buttons = init.buttons ?? 0;
      this.relatedTarget = init.relatedTarget ?? null;
      modifierFlags(this, init);
    }
  }

  class KeyboardEvent extends UIEvent {
    constructor(type, init = {}) {
      super(type, init);
      this.key = init.key ?? '';
      this.code = init.code ?? '';
      this.location = 0;
      this.repeat = !!init.repeat;
      this.isComposing = false;
      this.keyCode = init.keyCode ?? 0;
      this.charCode = type === 'keypress' ? this.keyCode : 0;
      this.which = this.keyCode;
      modifierFlags(this, init);
    }
  }

  class InputEvent extends UIEvent {
    constructor(type, init = {}) {
      super(type, init);
      this.data = init.data ?? null;
      this.inputType = init.inputType ?? 'insertText';
      this.isComposing = false;
    }
  }

  globalThis.Event = Event;
  globalThis.CustomEvent = CustomEvent;
  globalThis.MessageEvent = MessageEvent;
  globalThis.UIEvent = UIEvent;
  globalThis.MouseEvent = MouseEvent;
  globalThis.PointerEvent = MouseEvent;
  globalThis.KeyboardEvent = KeyboardEvent;
  globalThis.InputEvent = InputEvent;
  globalThis.FocusEvent = class FocusEvent extends UIEvent {};
  globalThis.ErrorEvent = class ErrorEvent extends Event {};
  globalThis.PromiseRejectionEvent = class PromiseRejectionEvent extends Event {};

  const keyOf = (node) =>
    node && typeof node.marNodeId === 'number' ? node.marNodeId : node;

  // Handles for the same DOM node are distinct objects, so listeners are keyed
  // by arena id in one shared registry rather than per handle.
  const registry = new Map();

  function listenerList(target, type, create) {
    const key = keyOf(target);
    let byType = registry.get(key);
    if (!byType) {
      if (!create) return null;
      byType = new Map();
      registry.set(key, byType);
    }
    let list = byType.get(type);
    if (!list) {
      if (!create) return null;
      list = [];
      byType.set(type, list);
    }
    return list;
  }

  function addEventListener(type, handler, options) {
    if (!handler) return;
    const list = listenerList(this, String(type), true);
    const once = typeof options === 'object' && options ? !!options.once : false;
    const capture =
      typeof options === 'boolean' ? options : !!(options && options.capture);
    if (list.some((l) => l.handler === handler && l.capture === capture)) return;
    list.push({ handler, once, capture });
  }

  function removeEventListener(type, handler, options) {
    const list = listenerList(this, String(type), false);
    if (!list) return;
    const capture =
      typeof options === 'boolean' ? options : !!(options && options.capture);
    const i = list.findIndex((l) => l.handler === handler && l.capture === capture);
    if (i >= 0) list.splice(i, 1);
  }

  function fireOn(target, event) {
    const list = listenerList(target, event.type, false);
    // Also honour the inline `onclick`-style property.
    const inline = target['on' + event.type];
    const handlers = list ? list.slice() : [];
    event.currentTarget = target;
    for (const entry of handlers) {
      if (event._stoppedImmediate) return;
      if (entry.once) removeEventListener.call(target, event.type, entry.handler, entry.capture);
      try {
        if (typeof entry.handler === 'function') entry.handler.call(target, event);
        else if (entry.handler && typeof entry.handler.handleEvent === 'function')
          entry.handler.handleEvent(event);
      } catch (e) {
        native.record_error('listener:' + event.type, String((e && e.stack) || e));
      }
    }
    if (typeof inline === 'function' && !event._stoppedImmediate) {
      try {
        inline.call(target, event);
      } catch (e) {
        native.record_error('on' + event.type, String((e && e.stack) || e));
      }
    }
  }

  function dispatchEvent(event) {
    if (!event || typeof event.type !== 'string') return true;
    event.target = event.target || this;
    // Capture phase, root down.
    const path = [];
    let n = this.parentNode;
    while (n) {
      path.push(n);
      n = n.parentNode;
    }
    if (event.bubbles !== false) {
      for (let i = path.length - 1; i >= 0; i--) {
        if (event._stopped) break;
        event.eventPhase = 1;
        fireOn(path[i], event);
      }
    }
    event.eventPhase = 2;
    if (!event._stopped) fireOn(this, event);
    if (event.bubbles) {
      event.eventPhase = 3;
      for (const ancestor of path) {
        if (event._stopped) break;
        fireOn(ancestor, event);
      }
      if (!event._stopped) fireOn(globalThis, event);
    }
    return !event.defaultPrevented;
  }

  method(NodeProto, 'addEventListener', addEventListener);
  method(NodeProto, 'removeEventListener', removeEventListener);
  method(NodeProto, 'dispatchEvent', dispatchEvent);

  globalThis.EventTarget = class EventTarget {
    addEventListener(...a) {
      return addEventListener.apply(this, a);
    }
    removeEventListener(...a) {
      return removeEventListener.apply(this, a);
    }
    dispatchEvent(...a) {
      return dispatchEvent.apply(this, a);
    }
  };

  // -- element extras ------------------------------------------------------

  // classList over the class attribute. Reads are live; writes go straight back.
  class DOMTokenList {
    constructor(el) {
      Object.defineProperty(this, '_el', { value: el });
    }
    get _tokens() {
      return (this._el.className || '').split(/\s+/).filter(Boolean);
    }
    _write(tokens) {
      this._el.className = tokens.join(' ');
    }
    get length() {
      return this._tokens.length;
    }
    get value() {
      return this._el.className || '';
    }
    item(i) {
      return this._tokens[i] ?? null;
    }
    contains(t) {
      return this._tokens.includes(String(t));
    }
    add(...ts) {
      const tokens = this._tokens;
      for (const t of ts) if (!tokens.includes(String(t))) tokens.push(String(t));
      this._write(tokens);
    }
    remove(...ts) {
      const drop = ts.map(String);
      this._write(this._tokens.filter((t) => !drop.includes(t)));
    }
    toggle(t, force) {
      const has = this.contains(t);
      const want = force === undefined ? !has : !!force;
      if (want) this.add(t);
      else this.remove(t);
      return want;
    }
    replace(a, b) {
      const tokens = this._tokens;
      const i = tokens.indexOf(String(a));
      if (i < 0) return false;
      tokens[i] = String(b);
      this._write(tokens);
      return true;
    }
    forEach(fn, thisArg) {
      this._tokens.forEach(fn, thisArg);
    }
    toString() {
      return this.value;
    }
    [Symbol.iterator]() {
      return this._tokens[Symbol.iterator]();
    }
  }

  define(NodeProto, 'classList', {
    get() {
      return new DOMTokenList(this);
    },
  });

  // style over the style attribute. There is no cascade and no layout, so this
  // reflects only what a script itself sets or the attribute already holds.
  const dashed = (p) => p.replace(/[A-Z]/g, (c) => '-' + c.toLowerCase());
  const camel = (p) => p.replace(/-([a-z])/g, (_, c) => c.toUpperCase());

  function parseStyle(text) {
    const out = new Map();
    for (const decl of String(text || '').split(';')) {
      const i = decl.indexOf(':');
      if (i < 0) continue;
      const name = decl.slice(0, i).trim();
      const value = decl.slice(i + 1).trim();
      if (name) out.set(name.toLowerCase(), value);
    }
    return out;
  }

  function styleProxy(el) {
    const read = () => parseStyle(el.getAttribute('style'));
    const write = (map) =>
      el.setAttribute(
        'style',
        [...map].map(([k, v]) => `${k}: ${v}`).join('; ')
      );
    const api = {
      getPropertyValue: (p) => read().get(dashed(String(p)).toLowerCase()) || '',
      setProperty: (p, v) => {
        const map = read();
        map.set(dashed(String(p)).toLowerCase(), String(v));
        write(map);
      },
      removeProperty: (p) => {
        const map = read();
        const key = dashed(String(p)).toLowerCase();
        const old = map.get(key) || '';
        map.delete(key);
        write(map);
        return old;
      },
      get cssText() {
        return el.getAttribute('style') || '';
      },
      set cssText(v) {
        el.setAttribute('style', String(v));
      },
    };
    return new Proxy(api, {
      get(target, prop) {
        if (prop in target) return target[prop];
        if (typeof prop !== 'string') return undefined;
        return read().get(dashed(prop).toLowerCase()) || '';
      },
      set(target, prop, value) {
        if (prop === 'cssText') {
          target.cssText = value;
          return true;
        }
        if (typeof prop === 'string') target.setProperty(dashed(prop), value);
        return true;
      },
    });
  }

  define(NodeProto, 'style', {
    get() {
      return styleProxy(this);
    },
  });

  // dataset over data-* attributes.
  define(NodeProto, 'dataset', {
    get() {
      const el = this;
      return new Proxy(
        {},
        {
          get: (_t, prop) =>
            typeof prop === 'string'
              ? el.getAttribute('data-' + dashed(prop)) ?? undefined
              : undefined,
          set: (_t, prop, value) => {
            el.setAttribute('data-' + dashed(String(prop)), String(value));
            return true;
          },
          has: (_t, prop) => el.hasAttribute('data-' + dashed(String(prop))),
          ownKeys: () =>
            el
              .getAttributeNames()
              .filter((n) => n.startsWith('data-'))
              .map((n) => camel(n.slice(5))),
          getOwnPropertyDescriptor: () => ({ enumerable: true, configurable: true }),
        }
      );
    },
  });

  // Common reflected properties. Each maps a JS property to an attribute.
  for (const [prop, attr] of [
    ['title', 'title'],
    ['lang', 'lang'],
    ['dir', 'dir'],
    ['alt', 'alt'],
    ['name', 'name'],
    ['type', 'type'],
    ['placeholder', 'placeholder'],
    ['rel', 'rel'],
    ['target', 'target'],
    ['content', 'content'],
  ]) {
    define(NodeProto, prop, {
      get() {
        return this.getAttribute(attr) ?? '';
      },
      set(v) {
        this.setAttribute(attr, String(v));
      },
    });
  }

  // URL-valued attributes resolve against the document, matching the DOM.
  for (const [prop, attr] of [
    ['href', 'href'],
    ['src', 'src'],
    ['action', 'action'],
  ]) {
    define(NodeProto, prop, {
      get() {
        const raw = this.getAttribute(attr);
        if (raw == null) return '';
        try {
          return new URL(raw, location.href).href;
        } catch {
          return raw;
        }
      },
      set(v) {
        this.setAttribute(attr, String(v));
      },
    });
  }

  // A link is also a parsed URL. `a.pathname`, `a.host` and the rest are how
  // a great deal of code asks "is this URL on my own site?" — axios does it
  // on every request — and an element that only reflects `href` answers
  // `undefined`, which the caller then calls `.charAt` on.
  for (const [prop, fallback] of [
    ['protocol', ''], ['host', ''], ['hostname', ''], ['port', ''],
    ['pathname', '/'], ['search', ''], ['hash', ''], ['origin', ''],
    ['username', ''], ['password', ''],
  ]) {
    define(NodeProto, prop, {
      get() {
        const attr = this.tagName === 'FORM' ? 'action' : 'href';
        const raw = this.getAttribute(attr);
        if (raw == null) return fallback;
        try {
          return new URL(raw, location.href)[prop] ?? fallback;
        } catch {
          return fallback;
        }
      },
      set(v) {
        const attr = this.tagName === 'FORM' ? 'action' : 'href';
        try {
          const url = new URL(this.getAttribute(attr) ?? '', location.href);
          url[prop] = v;
          this.setAttribute(attr, url.href);
        } catch {
          /* an element with no usable href keeps whatever it had */
        }
      },
    });
  }

  // Event handlers written into the markup.
  //
  // `onclick="doThing()"` is a handler, not just an attribute, and a page that
  // wires its buttons that way is otherwise inert: `fireOn` looks for the
  // property, and until it exists nothing connects the two. The source is
  // compiled on first read and kept, because a handler is one function object
  // across reads — a page that removes the listener it just read must get the
  // same one back.
  const compiled = new Map();
  for (const prop of [
    'onclick', 'ondblclick', 'onchange', 'oninput', 'onsubmit', 'onreset',
    'onload', 'onerror', 'onfocus', 'onblur',
    'onkeydown', 'onkeyup', 'onkeypress',
    'onmousedown', 'onmouseup', 'onmouseover', 'onmouseout', 'onmousemove',
    'oncontextmenu', 'onscroll', 'ontouchstart', 'ontouchend',
  ]) {
    define(NodeProto, prop, {
      get() {
        const key = keyOf(this) + ':' + prop;
        if (compiled.has(key)) return compiled.get(key);
        const source = this.getAttribute(prop);
        if (source == null) return null;
        let handler = null;
        try {
          // A browser runs the attribute with the element as `this` and the
          // event in scope, and swallows a syntax error rather than failing
          // the whole document.
          const body = new Function('event', source);
          handler = function (event) {
            return body.call(this, event);
          };
        } catch (e) {
          native.record_error(prop, String((e && e.message) || e));
        }
        compiled.set(key, handler);
        return handler;
      },
      set(v) {
        compiled.set(keyOf(this) + ':' + prop, typeof v === 'function' ? v : null);
      },
    });
  }

  // Boolean attributes.
  for (const prop of ['disabled', 'checked', 'readOnly', 'required', 'hidden', 'selected']) {
    const attr = dashed(prop);
    define(NodeProto, prop, {
      get() {
        return this.hasAttribute(attr);
      },
      set(v) {
        if (v) this.setAttribute(attr, '');
        else this.removeAttribute(attr);
      },
    });
  }

  // Form values live off-DOM, as they do in a real browser: typing into an
  // input does not change its value attribute.
  const values = new Map();
  define(NodeProto, 'value', {
    get() {
      const key = keyOf(this);
      if (values.has(key)) return values.get(key);
      if (this.tagName === 'TEXTAREA') return this.textContent;
      return this.getAttribute('value') ?? '';
    },
    set(v) {
      values.set(keyOf(this), String(v));
    },
  });

  // No layout engine: every box is zero-sized at the origin. Scripts that
  // branch on visibility take the "not visible" path, which is honest.
  //
  // A CDP client that clicks by coordinate needs boxes that differ, and gets
  // synthetic ones from `spatial` below — but only while it is the one asking.
  const zeroRect = () => ({
    x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0,
    width: 0, height: 0, toJSON() { return { ...this }; },
  });
  // -- a cascade, and a box to go with it -----------------------------------

  // Nothing here lays anything out, and nothing here is going to. But a page
  // that measures itself and finds zero concludes it has no room and renders
  // the collapsed branch — on a corpus of real sites that is the difference
  // between an article and an empty shell. So elements get a plausible box
  // instead of an honest zero.
  //
  // Plausible means: what the CSS actually says, where the CSS says anything.
  // That needs a cascade, and a cascade needs a selector engine — which this
  // project has, natively, in the same arena the nodes live in. So unlike the
  // usual shortcut of reading only the inline `style` attribute, rules from
  // `<style>` are matched too, and `.card { width: 320px }` is a 320-pixel box.
  const cascade = (() => {
    let rules = null;
    let builtFrom = -1;

    // Specificity, the three-number version, packed into one integer.
    // Approximate on purpose: `:is()` and `:where()` change the count and are
    // not unpacked here. Ordering nearly-right beats not ordering at all.
    const specificity = (selector) => {
      const ids = (selector.match(/#[\w-]+/g) || []).length;
      const classes = (selector.match(/[.:[][\w-]+/g) || []).length;
      const tags = (selector.match(/(^|[\s>+~])[a-zA-Z][\w-]*/g) || []).length;
      return ids * 10000 + classes * 100 + tags;
    };

    const parseDeclarations = (text) => {
      const out = {};
      for (const part of text.split(';')) {
        const at = part.indexOf(':');
        if (at < 0) continue;
        const name = part.slice(0, at).trim().toLowerCase();
        const value = part.slice(at + 1).trim();
        if (name && value) out[name] = value;
      }
      return out;
    };

    // A tolerant sweep rather than a CSS parser: comments out, at-rules that
    // are not `@media` skipped whole, everything else read as
    // `selectors { declarations }`. A rule this misreads is a rule that does
    // not apply, which is where we started.
    const parseSheet = (css, into) => {
      const text = css.replace(/\/\*[\s\S]*?\*\//g, '');
      let i = 0;
      while (i < text.length) {
        const open = text.indexOf('{', i);
        if (open < 0) break;
        const prelude = text.slice(i, open).trim();
        // Find the matching close brace, so nested blocks stay together.
        let depth = 1;
        let j = open + 1;
        while (j < text.length && depth > 0) {
          if (text[j] === '{') depth += 1;
          else if (text[j] === '}') depth -= 1;
          j += 1;
        }
        const body = text.slice(open + 1, j - 1);
        i = j;
        if (prelude.startsWith('@')) {
          // Only the conditional groups can contain rules that apply here.
          if (/^@(media|supports|layer|scope)\b/.test(prelude)) {
            const condition = prelude.replace(/^@\w+\s*/, '');
            const applies = !/^\(|^screen|^all|^only/.test(condition)
              || !condition
              || matchMedia(condition).matches
              || /^@(supports|layer|scope)/.test(prelude);
            if (applies) parseSheet(body, into);
          }
          continue;
        }
        const declarations = parseDeclarations(body);
        if (!Object.keys(declarations).length) continue;
        for (const selector of prelude.split(',')) {
          const trimmed = selector.trim();
          // A selector with a pseudo-element or a pseudo-class the engine
          // cannot match would throw on every element; drop those instead.
          if (!trimmed || trimmed.includes('::')) continue;
          into.push({ selector: trimmed, declarations, weight: specificity(trimmed) });
        }
      }
    };

    const build = () => {
      const sheets = document.querySelectorAll('style');
      // Cheap staleness check: a page that adds a stylesheet gets a rebuild,
      // and one that does not pays for the cascade exactly once.
      let fingerprint = sheets.length;
      for (const sheet of sheets) fingerprint += (sheet.textContent || '').length;
      if (rules && fingerprint === builtFrom) return rules;
      rules = [];
      for (const sheet of sheets) {
        try {
          parseSheet(sheet.textContent || '', rules);
        } catch (e) { /* one unreadable sheet is not the others' problem */ }
      }
      // Document order within a specificity, as the cascade requires.
      rules.forEach((r, i) => { r.order = i; });
      rules.sort((a, b) => a.weight - b.weight || a.order - b.order);
      builtFrom = fingerprint;
      return rules;
    };

    const cacheKey = Symbol('computed');
    return {
      for(el) {
        if (!el || el.nodeType !== 1) return {};
        const all = build();
        const cached = el[cacheKey];
        if (cached && cached.from === builtFrom) return cached.value;
        const value = {};
        for (const rule of all) {
          let hit = false;
          try { hit = el.matches(rule.selector); } catch (e) { hit = false; }
          if (hit) Object.assign(value, rule.declarations);
        }
        // The inline attribute outranks every rule.
        const inline = el.getAttribute && el.getAttribute('style');
        if (inline) Object.assign(value, parseDeclarations(inline));
        try { define(el, cacheKey, { value: { from: builtFrom, value }, configurable: true }); }
        catch (e) { /* a handle we cannot annotate simply recomputes */ }
        return value;
      },
    };
  })();

  // A length in CSS, resolved against what we can know. Percentages resolve
  // against the viewport rather than the parent, which is wrong in detail and
  // right in kind: a page asking whether it has room gets "yes".
  const cssLength = (value, axis) => {
    if (!value) return null;
    const text = String(value).trim();
    const m = /^(-?[\d.]+)(px|rem|em|pt|vw|vh|%)?$/.exec(text);
    if (!m) return null;
    const n = parseFloat(m[1]);
    if (!isFinite(n)) return null;
    const viewport = axis === 'width' ? innerWidth : innerHeight;
    switch (m[2]) {
      case 'rem': case 'em': return n * 16;
      case 'pt': return n * (4 / 3);
      case 'vw': return (n / 100) * innerWidth;
      case 'vh': return (n / 100) * innerHeight;
      case '%': return (n / 100) * viewport;
      default: return n;
    }
  };

  // Roughly how much room this element's own content would need. Not a layout:
  // a character count turned into lines at the viewport width, which is enough
  // to tell "this has content" from "this is empty".
  const contentBox = (el, axis) => {
    const text = (el.textContent || '').length;
    const kids = el.children ? el.children.length : 0;
    if (axis === 'width') {
      return Math.min(innerWidth, Math.max(text ? 200 : 0, kids ? innerWidth : 0));
    }
    const perLine = Math.max(20, Math.floor(innerWidth / 8));
    return Math.max(text ? Math.ceil(text / perLine) * 20 : 0, kids * 20);
  };

  const boxAxis = (el, axis) => {
    const style = cascade.for(el);
    if (style.display === 'none') return 0;
    const explicit = cssLength(style[axis], axis);
    if (explicit !== null) return explicit;
    const tag = el.tagName;
    if (tag === 'HTML' || tag === 'BODY') {
      return axis === 'width' ? innerWidth : Math.max(innerHeight, contentBox(el, axis));
    }
    // The presentational attributes, which for a picture or a frame are the
    // only size anyone ever writes.
    if (tag === 'IMG' || tag === 'IFRAME' || tag === 'CANVAS' || tag === 'VIDEO' || tag === 'EMBED') {
      const attr = parseFloat(el.getAttribute(axis));
      if (isFinite(attr)) return attr;
    }
    const min = cssLength(style[`min-${axis}`], axis);
    const content = contentBox(el, axis);
    return Math.max(min ?? 0, content, DEFAULT_BOX);
  };

  // Every element gets at least this. Small enough that nothing mistakes it
  // for a real measurement, large enough that "has any size at all" is true.
  const DEFAULT_BOX = 4;

  // Vertical position from depth and document order, so two elements are not
  // in the same place and something further down the page reads as lower.
  const documentPosition = (el) => {
    let depth = 0;
    let n = el;
    while (n && n.parentNode) { depth += 1; n = n.parentNode; }
    return depth * 24;
  };

  globalThis.__mar_cascade = (el) => cascade.for(el);

  const syntheticRect = (el) => {
    if (!el || el.nodeType !== 1) return zeroRect();
    const style = cascade.for(el);
    if (style.display === 'none' || style.visibility === 'hidden') return zeroRect();
    const width = boxAxis(el, 'width');
    const height = boxAxis(el, 'height');
    const y = documentPosition(el);
    return {
      x: 0, y, top: y, left: 0, right: width, bottom: y + height,
      width, height, toJSON() { return { ...this }; },
    };
  };

  // A client measuring for coordinates gets the tile registry; the page
  // measuring itself gets a box derived from its own CSS.
  method(NodeProto, 'getBoundingClientRect', function () {
    return spatial.enabled ? spatial.rect(this) : syntheticRect(this);
  });
  method(NodeProto, 'getClientRects', function () {
    if (spatial.enabled) return [spatial.rect(this)];
    const rect = syntheticRect(this);
    return rect.width || rect.height ? [rect] : [];
  });


  method(NodeProto, 'scrollIntoView', () => {});
  method(NodeProto, 'focus', function () {
    activeElement = this;
  });
  method(NodeProto, 'blur', function () {
    if (activeElement === this) activeElement = null;
  });
  method(NodeProto, 'click', function () {
    dispatchEvent.call(this, new Event('click', { bubbles: true, cancelable: true }));
  });
  method(NodeProto, 'submit', function () {
    dispatchEvent.call(this, new Event('submit', { bubbles: true, cancelable: true }));
  });
  // Sizes follow the same rule as rectangles: zero for the page, and the
  // synthetic tile for a client that is measuring. A client clips a click
  // point to the document's own size, so that one has to cover the tiles.
  const sized = (pick) =>
    function () {
      const root = keyOf(this) === keyOf(document) || this.tagName === 'HTML' || this.tagName === 'BODY';
      if (spatial.enabled) {
        const [w, h] = root ? spatial.extent() : [spatial.rect(this).width, spatial.rect(this).height];
        return pick(w, h);
      }
      if (this.nodeType !== 1) return 0;
      return Math.round(pick(boxAxis(this, 'width'), boxAxis(this, 'height')));
    };
  for (const p of ['offsetWidth', 'clientWidth', 'scrollWidth']) {
    define(NodeProto, p, { get: sized((w) => w), set: () => {} });
  }
  for (const p of ['offsetHeight', 'clientHeight', 'scrollHeight']) {
    define(NodeProto, p, { get: sized((_, h) => h), set: () => {} });
  }
  define(NodeProto, 'offsetTop', {
    get() { return this.nodeType === 1 ? Math.round(documentPosition(this)) : 0; },
    set: () => {},
  });
  define(NodeProto, 'offsetParent', {
    get() { return this.nodeType === 1 ? (this.parentElement ?? null) : null; },
  });
  for (const p of ['scrollTop', 'scrollLeft', 'offsetLeft']) {
    define(NodeProto, p, { get: () => 0, set: () => {} });
  }
  define(NodeProto, 'offsetParent', { get: () => null });

  method(NodeProto, 'normalize', () => {});
  method(NodeProto, 'after', function (...nodes) {
    const parent = this.parentNode;
    if (!parent) return;
    const next = this.nextSibling;
    for (const n of nodes) parent.insertBefore(toNode(n), next);
  });
  method(NodeProto, 'before', function (...nodes) {
    const parent = this.parentNode;
    if (!parent) return;
    for (const n of nodes) parent.insertBefore(toNode(n), this);
  });
  method(NodeProto, 'replaceWith', function (...nodes) {
    const parent = this.parentNode;
    if (!parent) return;
    for (const n of nodes) parent.insertBefore(toNode(n), this);
    this.remove();
  });
  method(NodeProto, 'replaceChildren', function (...nodes) {
    while (this.firstChild) this.removeChild(this.firstChild);
    for (const n of nodes) this.appendChild(toNode(n));
  });
  // The node itself is moved, not a copy of its markup: the caller keeps the
  // reference it passed in and expects that one to be in the tree.
  method(NodeProto, 'insertAdjacentElement', function (pos, el) {
    const where = String(pos).toLowerCase();
    if (where === 'beforebegin') this.parentNode?.insertBefore(el, this);
    else if (where === 'afterbegin') this.insertBefore(el, this.firstChild);
    else if (where === 'beforeend') this.appendChild(el);
    else if (where === 'afterend') this.parentNode?.insertBefore(el, this.nextSibling);
    else throw new SyntaxError(`insertAdjacentElement: bad position ${pos}`);
    return el;
  });
  method(NodeProto, 'insertAdjacentText', function (pos, text) {
    this.insertAdjacentHTML(pos, String(text).replace(/[&<>]/g, (c) =>
      ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' })[c]));
  });

  const toNode = (n) => (n instanceof Node ? n : document.createTextNode(String(n)));

  define(NodeProto, 'attributes', {
    get() {
      const el = this;
      const names = el.getAttributeNames();
      const list = names.map((name) => ({ name, value: el.getAttribute(name) }));
      list.getNamedItem = (n) => list.find((a) => a.name === n) ?? null;
      return list;
    },
  });

  define(NodeProto, 'namespaceURI', { get: () => 'http://www.w3.org/1999/xhtml' });
  define(NodeProto, 'isConnected', {
    get() {
      let n = this;
      while (n.parentNode) n = n.parentNode;
      // Two handles for one node are distinct objects, so `===` on the handles
      // answers "not connected" for everything. Compare arena ids instead.
      return keyOf(n) === keyOf(document) || keyOf(n) === keyOf(document.documentElement);
    },
  });

  // The DOM interface names, as `instanceof` targets. Nodes come from the
  // document and none of these can be constructed, but scripts branch on them
  // constantly and a missing global is a ReferenceError that stops a script
  // dead. Membership is decided by what a node is, because handles share one
  // prototype and a prototype chain could not tell them apart.
  const domInterface = (name, holds) => {
    const ctor = function () {
      throw new TypeError(`Illegal constructor: ${name}`);
    };
    define(ctor, 'name', { value: name });
    define(ctor, Symbol.hasInstance, { value: holds });
    ctor.prototype = NodeProto;
    globalThis[name] = ctor;
  };
  const isType = (type) => (v) => !!v && v.nodeType === type;
  // `EventTarget` is not in this list: it is defined above as a real class,
  // because a page that writes `class Bus extends EventTarget` has to be able
  // to construct one.
  domInterface('Element', isType(1));
  domInterface('HTMLElement', isType(1));
  domInterface('Text', isType(3));
  domInterface('Comment', isType(8));
  domInterface('CharacterData', (v) => !!v && (v.nodeType === 3 || v.nodeType === 8));
  domInterface('Document', isType(9));
  domInterface('HTMLDocument', isType(9));
  domInterface('DocumentFragment', isType(11));
  domInterface('DocumentType', isType(10));
  // Nothing here attaches shadow trees, so nothing is in one. The global still
  // has to exist: a library walking up from a node tests the root against it,
  // and an undefined right-hand side of `instanceof` is a TypeError.
  domInterface('ShadowRoot', () => false);
  // Nothing here parses SVG into SVG-aware nodes, so nothing is one.
  domInterface('SVGElement', () => false);
  domInterface('SVGSVGElement', () => false);

  // One interface per tag, because that is how scripts ask. React's scheduler
  // alone tests HTMLDivElement, HTMLBRElement, HTMLBodyElement and ShadowRoot,
  // and a single missing name there throws inside the render and takes the
  // whole application down with it — a blank page from one absent global.
  const TAG_INTERFACES = {
    HTMLAnchorElement: 'A', HTMLAreaElement: 'AREA', HTMLAudioElement: 'AUDIO',
    HTMLBaseElement: 'BASE', HTMLBodyElement: 'BODY', HTMLBRElement: 'BR',
    HTMLButtonElement: 'BUTTON', HTMLCanvasElement: 'CANVAS', HTMLDataElement: 'DATA',
    HTMLDataListElement: 'DATALIST', HTMLDetailsElement: 'DETAILS', HTMLDialogElement: 'DIALOG',
    HTMLDivElement: 'DIV', HTMLEmbedElement: 'EMBED', HTMLFieldSetElement: 'FIELDSET',
    HTMLFormElement: 'FORM', HTMLHeadElement: 'HEAD', HTMLHRElement: 'HR',
    HTMLHtmlElement: 'HTML', HTMLIFrameElement: 'IFRAME', HTMLImageElement: 'IMG',
    HTMLInputElement: 'INPUT', HTMLLabelElement: 'LABEL', HTMLLegendElement: 'LEGEND',
    HTMLLIElement: 'LI', HTMLLinkElement: 'LINK', HTMLMapElement: 'MAP',
    HTMLMenuElement: 'MENU', HTMLMetaElement: 'META', HTMLMeterElement: 'METER',
    HTMLObjectElement: 'OBJECT', HTMLOListElement: 'OL', HTMLOptGroupElement: 'OPTGROUP',
    HTMLOptionElement: 'OPTION', HTMLOutputElement: 'OUTPUT', HTMLParagraphElement: 'P',
    HTMLPictureElement: 'PICTURE', HTMLPreElement: 'PRE', HTMLProgressElement: 'PROGRESS',
    HTMLScriptElement: 'SCRIPT', HTMLSelectElement: 'SELECT', HTMLSlotElement: 'SLOT',
    HTMLSourceElement: 'SOURCE', HTMLSpanElement: 'SPAN', HTMLStyleElement: 'STYLE',
    HTMLTableCaptionElement: 'CAPTION', HTMLTableColElement: 'COL', HTMLTableElement: 'TABLE',
    HTMLTableRowElement: 'TR', HTMLTemplateElement: 'TEMPLATE', HTMLTextAreaElement: 'TEXTAREA',
    HTMLTimeElement: 'TIME', HTMLTitleElement: 'TITLE', HTMLTrackElement: 'TRACK',
    HTMLUListElement: 'UL', HTMLVideoElement: 'VIDEO',
  };
  for (const [name, tag] of Object.entries(TAG_INTERFACES)) {
    domInterface(name, (v) => !!v && v.nodeType === 1 && v.tagName === tag);
  }
  // Interfaces one tag name cannot answer for.
  const anyOf = (...tags) => (v) => !!v && v.nodeType === 1 && tags.includes(v.tagName);
  domInterface('HTMLHeadingElement', anyOf('H1', 'H2', 'H3', 'H4', 'H5', 'H6'));
  domInterface('HTMLTableCellElement', anyOf('TD', 'TH'));
  domInterface('HTMLTableSectionElement', anyOf('THEAD', 'TBODY', 'TFOOT'));
  domInterface('HTMLQuoteElement', anyOf('BLOCKQUOTE', 'Q'));
  domInterface('HTMLModElement', anyOf('INS', 'DEL'));
  domInterface('HTMLMediaElement', anyOf('AUDIO', 'VIDEO'));
  domInterface('HTMLUnknownElement', () => false);

  // The constructible ones. `new Image()` is how half the web preloads a
  // picture, and it is a plain `document.createElement` underneath.
  const elementConstructor = (name, tag, apply) => {
    const ctor = function (...args) {
      const el = native.create_element(tag);
      if (apply) apply(el, args);
      return el;
    };
    define(ctor, 'name', { value: name });
    define(ctor, Symbol.hasInstance, {
      value: (v) => !!v && v.nodeType === 1 && v.tagName === tag.toUpperCase(),
    });
    ctor.prototype = NodeProto;
    globalThis[name] = ctor;
  };
  elementConstructor('Image', 'img', (el, [w, h]) => {
    if (w !== undefined) el.setAttribute('width', String(w));
    if (h !== undefined) el.setAttribute('height', String(h));
  });
  elementConstructor('Audio', 'audio', (el, [src]) => {
    if (src !== undefined) el.setAttribute('src', String(src));
  });
  elementConstructor('Option', 'option', (el, [text, value, _def, selected]) => {
    if (text !== undefined) el.textContent = String(text);
    if (value !== undefined) el.setAttribute('value', String(value));
    if (selected) el.setAttribute('selected', '');
  });

  // The nodeType constants, which scripts compare against by name.
  for (const [name, value] of Object.entries({
    ELEMENT_NODE: 1, ATTRIBUTE_NODE: 2, TEXT_NODE: 3, CDATA_SECTION_NODE: 4,
    PROCESSING_INSTRUCTION_NODE: 7, COMMENT_NODE: 8, DOCUMENT_NODE: 9,
    DOCUMENT_TYPE_NODE: 10, DOCUMENT_FRAGMENT_NODE: 11,
  })) {
    define(Node, name, { value });
    define(NodeProto, name, { value });
  }

  // -- document ------------------------------------------------------------

  let activeElement = null;

  const documentNode = native.root();
  const document = documentNode;

  define(document, 'documentElement', { get: () => native.document_element() });
  define(document, 'body', { get: () => native.body() });
  define(document, 'head', { get: () => native.head() });
  define(document, 'title', {
    get: () => native.title(),
    set(v) {
      let t = document.querySelector('title');
      if (!t) {
        t = native.create_element('title');
        (native.head() || document.documentElement).appendChild(t);
      }
      t.textContent = String(v);
    },
  });
  define(document, 'readyState', { get: () => native.ready_state() });
  define(document, 'cookie', {
    get: () => native.cookie_get(),
    set: (v) => native.cookie_set(String(v)),
  });
  define(document, 'activeElement', { get: () => activeElement || native.body() });
  define(document, 'referrer', { get: () => native.referrer() });
  define(document, 'characterSet', { get: () => 'UTF-8' });
  define(document, 'compatMode', { get: () => 'CSS1Compat' });
  define(document, 'currentScript', { get: () => native.current_script(), configurable: true });
  define(document, 'scrollingElement', { get: () => native.document_element() });
  define(document, 'defaultView', { get: () => globalThis });
  define(document, 'forms', { get: () => document.querySelectorAll('form') });
  define(document, 'images', { get: () => document.querySelectorAll('img') });
  define(document, 'links', { get: () => document.querySelectorAll('a[href], area[href]') });
  define(document, 'scripts', { get: () => document.querySelectorAll('script') });
  define(document, 'URL', { get: () => location.href });
  define(document, 'documentURI', { get: () => location.href });
  define(document, 'location', { get: () => location, set: (v) => native.navigate(String(v)) });

  method(document, 'createElement', (n) => native.create_element(n));
  method(document, 'createElementNS', (_ns, n) => native.create_element(n));
  method(document, 'createTextNode', (t) => native.create_text_node(String(t)));
  method(document, 'createComment', (t) => native.create_comment(String(t)));
  method(document, 'createDocumentFragment', () => native.create_fragment());
  method(document, 'getElementById', (id) => native.get_element_by_id(String(id)));
  method(document, 'write', (...s) => {
    // document.write after parsing would replace the page in a real browser.
    // Appending is the safer reading of what the script intended.
    const body = native.body();
    if (body) body.insertAdjacentHTML('beforeend', s.join(''));
  });
  method(document, 'writeln', (...s) => document.write(...s, '\n'));
  method(document, 'open', () => document);
  method(document, 'close', () => {});
  // DOMPurify and every other sanitiser start by asking for a blank document
  // to parse into, and fall over on the spot when this is missing. There is
  // one document here, so the "new" one is a detached subtree of it — which is
  // what the callers do with it anyway.
  define(document, 'implementation', {
    get: () => ({
      hasFeature: () => true,
      createHTMLDocument(title) {
        const doc = native.create_element('html');
        doc.innerHTML = '<head></head><body></body>';
        const head = doc.querySelector('head');
        const body = doc.querySelector('body');
        if (title !== undefined) {
          const el = native.create_element('title');
          el.textContent = String(title);
          head.appendChild(el);
        }
        define(doc, 'head', { get: () => head, configurable: true });
        define(doc, 'body', { get: () => body, configurable: true });
        define(doc, 'documentElement', { get: () => doc, configurable: true });
        method(doc, 'createElement', (n) => native.create_element(n));
        method(doc, 'createTextNode', (t) => native.create_text_node(String(t)));
        method(doc, 'createDocumentFragment', () => native.create_fragment());
        method(doc, 'getElementById', (id) => doc.querySelector(`[id="${String(id).replace(/"/g, '\\"')}"]`));
        return doc;
      },
      createDocument() { return this.createHTMLDocument(); },
      createDocumentType: (name) => ({ name, publicId: '', systemId: '' }),
    }),
  });

  method(document, 'importNode', (n, deep) => n.cloneNode(deep));
  method(document, 'adoptNode', (n) => n);
  method(document, 'createRange', () => ({
    setStart() {}, setEnd() {}, selectNodeContents() {},
    cloneContents: () => native.create_fragment(),
    createContextualFragment(html) {
      const t = native.create_element('template');
      t.innerHTML = html;
      const f = native.create_fragment();
      while (t.firstChild) f.appendChild(t.firstChild);
      return f;
    },
  }));
  method(document, 'createTreeWalker', (root) => {
    const nodes = [];
    (function walk(n) {
      for (const c of n.childNodes) {
        nodes.push(c);
        walk(c);
      }
    })(root);
    let i = -1;
    return {
      currentNode: root,
      nextNode: () => (++i < nodes.length ? (this.currentNode = nodes[i]) : null),
      previousNode: () => (--i >= 0 ? nodes[i] : null),
    };
  });
  method(document, 'elementFromPoint', () => null);
  method(document, 'hasFocus', () => true);
  method(document, 'execCommand', () => false);
  method(document, 'evaluate', () => ({ iterateNext: () => null, snapshotLength: 0 }));
  globalThis.document = document;

  // -- window --------------------------------------------------------------

  const loc = native.location();
  const location = {
    ...loc,
    assign: (u) => native.navigate(String(u)),
    replace: (u) => native.navigate(String(u)),
    // A reload is a navigation to the same URL. Challenge pages depend on it:
    // they compute something, set a cookie, and reload to be served the real
    // page. The host decides whether to follow it, as with any navigation.
    reload: () => native.navigate(loc.href),
    toString: () => loc.href,
  };
  // Assigning location.href is a navigation request, not a mutation.
  Object.defineProperty(location, 'href', {
    get: () => loc.href,
    set: (v) => native.navigate(String(v)),
    configurable: true,
  });
  globalThis.location = location;

  const [innerWidth, innerHeight] = native.viewport();
  const ua = native.user_agent();

  globalThis.navigator = {
    userAgent: ua,
    appVersion: ua.replace('Mozilla/', ''),
    platform: 'MacIntel',
    vendor: 'Google Inc.',
    language: 'en-US',
    languages: ['en-US', 'en'],
    onLine: true,
    cookieEnabled: true,
    // A plain "0 cores, no touch" profile is itself a fingerprint; these are
    // ordinary values for the user agent we claim.
    hardwareConcurrency: 8,
    deviceMemory: 8,
    maxTouchPoints: 0,
    webdriver: false,
    doNotTrack: null,
    plugins: { length: 0, item: () => null, namedItem: () => null },
    mimeTypes: { length: 0 },
    javaEnabled: () => false,
    sendBeacon: () => true,
    clipboard: { writeText: () => Promise.resolve() },
    permissions: { query: () => Promise.resolve({ state: 'prompt' }) },
    serviceWorker: { register: () => Promise.reject(new Error('unsupported')) },
    userAgentData: {
      brands: [{ brand: 'Chromium', version: '140' }],
      mobile: false,
      platform: 'macOS',
    },
  };

  globalThis.screen = {
    width: innerWidth, height: innerHeight,
    availWidth: innerWidth, availHeight: innerHeight,
    colorDepth: 24, pixelDepth: 24,
    orientation: { type: 'landscape-primary', angle: 0 },
  };

  globalThis.innerWidth = innerWidth;
  globalThis.innerHeight = innerHeight;
  globalThis.outerWidth = innerWidth;
  globalThis.outerHeight = innerHeight;
  globalThis.devicePixelRatio = 1;
  globalThis.scrollX = 0;
  globalThis.scrollY = 0;
  globalThis.pageXOffset = 0;
  globalThis.pageYOffset = 0;
  globalThis.name = '';
  globalThis.closed = false;
  globalThis.isSecureContext = location.protocol === 'https:';
  globalThis.origin = loc.origin;

  globalThis.scrollTo = () => {};
  globalThis.scrollBy = () => {};
  globalThis.scroll = () => {};
  globalThis.resizeTo = () => {};
  globalThis.moveTo = () => {};
  globalThis.focus = () => {};
  globalThis.blur = () => {};
  globalThis.print = () => {};
  globalThis.stop = () => {};
  globalThis.open = () => null;
  globalThis.close = () => {};
  globalThis.alert = (m) => native.log_log('alert: ' + m);
  globalThis.confirm = () => false;
  globalThis.prompt = () => null;
  globalThis.postMessage = (data) => {
    setTimeout(() => dispatchEvent.call(globalThis, new MessageEvent('message', { data })), 0);
  };

  // Computed style is the cascade plus the inline attribute. `__mar_cascade`
  // is installed further down, once the selector machinery it needs exists; a
  // script that asks before then still gets the inline answer.
  globalThis.getComputedStyle = (el) => {
    const inline = styleProxy(el);
    const resolved = globalThis.__mar_cascade ? globalThis.__mar_cascade(el) : {};
    return new Proxy(inline, {
      get(t, p) {
        if (p === 'getPropertyValue') {
          return (n) => t.getPropertyValue(n) || resolved[String(n).toLowerCase()] || '';
        }
        const own = t[p];
        if (own !== undefined && own !== '') return own;
        if (typeof p !== 'string') return own;
        // camelCase to the property name the sheet used.
        const dashed = p.replace(/[A-Z]/g, (c) => '-' + c.toLowerCase());
        return resolved[dashed] ?? resolved[p] ?? own;
      },
    });
  };

  globalThis.matchMedia = (query) => {
    const q = String(query);
    // Answer from the reported viewport so responsive code picks one branch.
    let matches = false;
    const min = /min-width:\s*(\d+)/.exec(q);
    const max = /max-width:\s*(\d+)/.exec(q);
    if (min) matches = innerWidth >= Number(min[1]);
    else if (max) matches = innerWidth <= Number(max[1]);
    else if (/prefers-color-scheme:\s*light/.test(q)) matches = true;
    else if (/prefers-reduced-motion:\s*no-preference/.test(q)) matches = true;
    else if (/\(hover:\s*hover\)/.test(q)) matches = true;
    return {
      media: q, matches, onchange: null,
      addListener() {}, removeListener() {},
      addEventListener() {}, removeEventListener() {},
      dispatchEvent: () => false,
    };
  };

  globalThis.history = {
    length: 1, scrollRestoration: 'auto', state: null,
    pushState(state) { this.state = state; },
    replaceState(state) { this.state = state; },
    back() {}, forward() {}, go() {},
  };

  const makeStorage = (isSession) => {
    const api = {
      getItem: (k) => native.storage_get(isSession, String(k)) ?? null,
      setItem: (k, v) => native.storage_set(isSession, String(k), String(v)),
      removeItem: (k) => native.storage_remove(isSession, String(k)),
      clear: () => native.storage_clear(isSession),
      key: (i) => native.storage_keys(isSession)[i] ?? null,
      get length() {
        return native.storage_keys(isSession).length;
      },
    };
    // Bracket access (`localStorage.foo`) is as common as getItem.
    return new Proxy(api, {
      get: (t, p) =>
        p in t ? t[p] : typeof p === 'string' ? t.getItem(p) ?? undefined : undefined,
      set: (t, p, v) => (typeof p === 'string' && !(p in t) ? (t.setItem(p, v), true) : true),
      has: (t, p) => p in t || native.storage_keys(isSession).includes(String(p)),
      deleteProperty: (t, p) => (t.removeItem(p), true),
      ownKeys: () => native.storage_keys(isSession),
      getOwnPropertyDescriptor: () => ({ enumerable: true, configurable: true }),
    });
  };
  globalThis.localStorage = makeStorage(false);
  globalThis.sessionStorage = makeStorage(true);

  globalThis.addEventListener = (...a) => addEventListener.apply(globalThis, a);
  globalThis.removeEventListener = (...a) => removeEventListener.apply(globalThis, a);
  globalThis.dispatchEvent = (...a) => dispatchEvent.apply(globalThis, a);
  globalThis.window = globalThis;
  globalThis.self = globalThis;
  globalThis.top = globalThis;
  globalThis.parent = globalThis;
  globalThis.frames = globalThis;
  globalThis.frameElement = null;
  globalThis.performance = {
    now: () => native.now(),
    timeOrigin: 0,
    mark: () => {}, measure: () => {},
    getEntries: () => [], getEntriesByName: () => [], getEntriesByType: () => [],
    clearMarks: () => {}, clearMeasures: () => {},
    timing: { navigationStart: 0 },
  };

  // Observers never fire: nothing is painted, resized or scrolled. They must
  // still construct, because widely used libraries build them at import time.
  const inertObserver = (extra = {}) =>
    class {
      constructor(cb) { this._cb = cb; Object.assign(this, extra); }
      observe() {} unobserve() {} disconnect() {}
      takeRecords() { return []; }
    };
  // An intersection cannot be computed without layout, but an observer that
  // never calls back is worse than one that answers "not visible": scripts
  // await that callback, and a CDP client asks whether an element is in the
  // viewport before it clicks. One record is delivered per observed target,
  // reporting what this engine does know — an element is visible exactly when
  // the spatial index has given it a box.
  globalThis.IntersectionObserver = class IntersectionObserver {
    constructor(cb) {
      this._cb = cb;
      this.root = null;
      this.rootMargin = '0px';
      this.thresholds = [0];
    }
    observe(target) {
      setTimeout(() => {
        const seen = spatial.enabled;
        const rect = seen ? spatial.rect(target) : zeroRect();
        try {
          this._cb([{
            target,
            time: native.now(),
            isIntersecting: seen,
            intersectionRatio: seen ? 1 : 0,
            boundingClientRect: rect,
            intersectionRect: seen ? rect : zeroRect(),
            rootBounds: null,
          }], this);
        } catch (e) {
          native.record_error('IntersectionObserver', String((e && e.stack) || e));
        }
      }, 0);
    }
    unobserve() {}
    disconnect() {}
    takeRecords() { return []; }
  };
  globalThis.ResizeObserver = inertObserver();
  globalThis.PerformanceObserver = inertObserver();

  // MutationObserver is different: scripts genuinely wait on it to learn that
  // their own DOM writes landed. Deliver a coarse record on the next tick.
  globalThis.MutationObserver = class MutationObserver {
    constructor(cb) { this._cb = cb; this._targets = []; }
    observe(target) {
      this._targets.push(target);
      setTimeout(() => {
        try {
          this._cb(
            [{ type: 'childList', target, addedNodes: [], removedNodes: [],
               attributeName: null, oldValue: null }],
            this
          );
        } catch (e) {
          native.record_error('MutationObserver', String((e && e.stack) || e));
        }
      }, 0);
    }
    disconnect() { this._targets = []; }
    takeRecords() { return []; }
  };

  // -- fetch and XHR -------------------------------------------------------

  class Headers {
    constructor(init) {
      this._m = new Map();
      if (init instanceof Headers) for (const [k, v] of init._m) this._m.set(k, v);
      else if (Array.isArray(init)) for (const [k, v] of init) this.set(k, v);
      else if (init && typeof init === 'object')
        for (const k of Object.keys(init)) this.set(k, init[k]);
    }
    get(k) { return this._m.get(String(k).toLowerCase()) ?? null; }
    set(k, v) { this._m.set(String(k).toLowerCase(), String(v)); }
    append(k, v) {
      const key = String(k).toLowerCase();
      const cur = this._m.get(key);
      this._m.set(key, cur ? `${cur}, ${v}` : String(v));
    }
    has(k) { return this._m.has(String(k).toLowerCase()); }
    delete(k) { this._m.delete(String(k).toLowerCase()); }
    forEach(fn, t) { this._m.forEach((v, k) => fn.call(t, v, k, this)); }
    keys() { return this._m.keys(); }
    values() { return this._m.values(); }
    entries() { return this._m.entries(); }
    [Symbol.iterator]() { return this._m[Symbol.iterator](); }
    toObject() { return Object.fromEntries(this._m); }
  }
  globalThis.Headers = Headers;

  class Response {
    constructor(body, init = {}) {
      this._body = body ?? '';
      this.status = init.status ?? 200;
      this.statusText = init.statusText ?? '';
      this.ok = this.status >= 200 && this.status < 300;
      this.headers = new Headers(init.headers);
      this.url = init.url ?? '';
      this.redirected = false;
      this.type = 'basic';
      this.bodyUsed = false;
    }
    text() { this.bodyUsed = true; return Promise.resolve(this._body); }
    json() {
      this.bodyUsed = true;
      try { return Promise.resolve(JSON.parse(this._body)); }
      catch (e) { return Promise.reject(e); }
    }
    // The body arrives decoded, so the byte views are re-encoded from the
    // text rather than being the bytes off the wire. That is exact for
    // anything a page then decodes as UTF-8, which is what a page fetching
    // JSON, a template or a translation file is doing.
    bytes() { this.bodyUsed = true; return Promise.resolve(new TextEncoder().encode(this._body)); }
    arrayBuffer() { return this.bytes().then((b) => b.buffer); }
    blob() {
      this.bodyUsed = true;
      const text = this._body;
      return Promise.resolve(new Blob([text], { type: this.headers.get('content-type') || '' }));
    }
    formData() {
      return this.text().then((t) => {
        const form = new FormData();
        for (const [k, v] of new URLSearchParams(t)) form.append(k, v);
        return form;
      });
    }
    // A single-chunk stream, so `for await (const c of res.body)` terminates
    // instead of hanging on a reader that never resolves.
    get body() {
      const text = this._body;
      if (this._stream) return this._stream;
      this._stream = new ReadableStream({
        start(controller) {
          if (text) controller.enqueue(new TextEncoder().encode(text));
          controller.close();
        },
      });
      return this._stream;
    }
    clone() { return new Response(this._body, this); }
  }
  globalThis.Response = Response;

  class Request {
    constructor(input, init = {}) {
      this.url = typeof input === 'string' ? input : input.url;
      this.method = (init.method || 'GET').toUpperCase();
      this.headers = new Headers(init.headers);
      this.body = init.body ?? null;
      this.credentials = init.credentials || 'same-origin';
      this.mode = init.mode || 'cors';
    }
  }
  globalThis.Request = Request;

  function doRequest(method, url, headers, body) {
    return native.request(String(method).toUpperCase(), String(url), headers, body ?? null);
  }

  // The asynchronous path. A page that writes `Promise.all([fetch(a), fetch(b)])`
  // means two requests in flight, not two in a row, and on a page with a
  // hundred of them the difference is most of the wall clock. The host starts
  // the request and hands back an id; the settle loop delivers the answer here
  // when it lands, which is also what keeps the page from being declared
  // settled while it is still waiting.
  const awaitingResponse = new Map();
  globalThis.__mar_deliver = function (id, raw) {
    const settle = awaitingResponse.get(id);
    if (!settle) return;
    awaitingResponse.delete(id);
    settle(raw);
  };
  function requestAsync(method, url, headers, body) {
    return new Promise((resolve) => {
      let id;
      try {
        id = native.request_start(String(method).toUpperCase(), String(url), headers, body ?? null);
      } catch (e) {
        resolve({ ok: false, status: 0, error: String(e) });
        return;
      }
      awaitingResponse.set(id, resolve);
    });
  }

  globalThis.fetch = (input, init = {}) => {
    const req = input instanceof Request ? input : new Request(input, init);
    const headers = new Headers(init.headers || req.headers).toObject();
    const bodyText = init.body ?? req.body;
    return requestAsync(req.method, req.url, headers,
      bodyText == null ? null : String(bodyText)).then((raw) => {
      if (raw.error && raw.status === 0) {
        throw new TypeError('Failed to fetch: ' + raw.error);
      }
      return new Response(raw.body ?? '', {
        status: raw.status, statusText: raw.statusText,
        headers: raw.headers, url: raw.url ?? req.url,
      });
    });
  };

  globalThis.XMLHttpRequest = class XMLHttpRequest {
    constructor() {
      this.readyState = 0;
      this.status = 0;
      this.statusText = '';
      this.responseText = '';
      this.response = '';
      this.responseType = '';
      this.responseURL = '';
      this.timeout = 0;
      this.withCredentials = false;
      this.upload = { addEventListener() {}, removeEventListener() {} };
      this._headers = {};
      this._responseHeaders = {};
      this._listeners = {};
    }
    open(method, url, async = true) {
      this._method = method;
      this._url = url;
      this._async = async !== false;
      this.readyState = 1;
      this._fire('readystatechange');
    }
    setRequestHeader(k, v) { this._headers[String(k)] = String(v); }
    getResponseHeader(k) { return this._responseHeaders[String(k).toLowerCase()] ?? null; }
    getAllResponseHeaders() {
      return Object.entries(this._responseHeaders).map(([k, v]) => `${k}: ${v}`).join('\r\n');
    }
    overrideMimeType() {}
    abort() { this.readyState = 0; this._fire('abort'); }
    addEventListener(t, fn) { (this._listeners[t] ||= []).push(fn); }
    removeEventListener(t, fn) {
      const l = this._listeners[t];
      if (l) this._listeners[t] = l.filter((f) => f !== fn);
    }
    _fire(type) {
      const ev = new Event(type);
      ev.target = this;
      for (const fn of this._listeners[type] || []) {
        try { fn.call(this, ev); } catch (e) { native.record_error('xhr:' + type, String(e)); }
      }
      const inline = this['on' + type];
      if (typeof inline === 'function') {
        try { inline.call(this, ev); } catch (e) { native.record_error('xhr:on' + type, String(e)); }
      }
    }
    send(body) {
      // A synchronous XHR blocks the page in a browser too, so it blocks here.
      // Everything else goes through the asynchronous path and overlaps.
      if (this._async === false) {
        let raw;
        try {
          raw = doRequest(this._method || 'GET', this._url, this._headers, body ?? null);
        } catch (e) {
          this._fire('error');
          return;
        }
        this._complete(raw);
        return;
      }
      requestAsync(this._method || 'GET', this._url, this._headers, body ?? null)
        .then((raw) => this._complete(raw));
    }
    _complete(raw) {
      this.status = raw.status ?? 0;
      this.statusText = raw.statusText ?? '';
      this.responseURL = raw.url ?? this._url;
      this.responseText = raw.body ?? '';
      this._responseHeaders = raw.headers ?? {};
      this.response =
        this.responseType === 'json'
          ? (() => { try { return JSON.parse(this.responseText); } catch { return null; } })()
          : this.responseText;
      this.readyState = 4;
      this._fire('readystatechange');
      this._fire(this.status === 0 ? 'error' : 'load');
      this._fire('loadend');
    }
  };

  globalThis.WebSocket = class WebSocket {
    constructor(url) {
      this.url = url;
      this.readyState = 3; // CLOSED: there is no socket layer.
      native.record_error('WebSocket', 'WebSocket is not supported: ' + url);
    }
    send() {} close() {}
    addEventListener() {} removeEventListener() {}
  };
  globalThis.EventSource = class EventSource {
    constructor(url) { this.url = url; this.readyState = 2; }
    close() {} addEventListener() {} removeEventListener() {}
  };
  globalThis.Worker = class Worker {
    constructor(url) { native.record_error('Worker', 'Worker is not supported: ' + url); }
    postMessage() {} terminate() {}
    addEventListener() {} removeEventListener() {}
  };

  // -- URL and URLSearchParams ---------------------------------------------

  // Parsing is done natively; these classes are the WHATWG surface over it.
  class URLSearchParams {
    constructor(init) {
      this._p = [];
      if (typeof init === 'string') {
        const parsed = native.parse_url('http://x/?' + init.replace(/^\?/, ''), null);
        if (parsed.ok) this._p = parsed.pairs.map(([k, v]) => [k, v]);
      } else if (init instanceof URLSearchParams) {
        this._p = init._p.map((e) => e.slice());
      } else if (Array.isArray(init)) {
        this._p = init.map(([k, v]) => [String(k), String(v)]);
      } else if (init && typeof init === 'object') {
        this._p = Object.keys(init).map((k) => [k, String(init[k])]);
      }
    }
    // Set by URL so mutations write back into the parent URL.
    _onchange() {}
    append(k, v) { this._p.push([String(k), String(v)]); this._onchange(); }
    set(k, v) {
      const key = String(k);
      const i = this._p.findIndex((e) => e[0] === key);
      if (i < 0) this._p.push([key, String(v)]);
      else { this._p[i][1] = String(v); this._p = this._p.filter((e, j) => j === i || e[0] !== key); }
      this._onchange();
    }
    get(k) { const e = this._p.find((e) => e[0] === String(k)); return e ? e[1] : null; }
    getAll(k) { return this._p.filter((e) => e[0] === String(k)).map((e) => e[1]); }
    has(k) { return this._p.some((e) => e[0] === String(k)); }
    delete(k) { this._p = this._p.filter((e) => e[0] !== String(k)); this._onchange(); }
    sort() { this._p.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)); this._onchange(); }
    forEach(fn, t) { for (const [k, v] of this._p.slice()) fn.call(t, v, k, this); }
    keys() { return this._p.map((e) => e[0])[Symbol.iterator](); }
    values() { return this._p.map((e) => e[1])[Symbol.iterator](); }
    entries() { return this._p.map((e) => e.slice())[Symbol.iterator](); }
    [Symbol.iterator]() { return this.entries(); }
    get size() { return this._p.length; }
    toString() { return native.encode_query(this._p); }
  }
  globalThis.URLSearchParams = URLSearchParams;

  class URL {
    constructor(input, base) {
      const parsed = native.parse_url(String(input), base == null ? null : String(base));
      if (!parsed.ok) throw new TypeError(`Invalid URL: ${input}`);
      this._apply(parsed);
    }
    _apply(parsed) {
      for (const k of ['href','protocol','host','hostname','port','pathname',
                       'search','hash','origin','username','password']) {
        this[k] = parsed[k];
      }
      const params = new URLSearchParams();
      params._p = parsed.pairs.map(([k, v]) => [k, v]);
      // Editing searchParams must update search and href, as in a browser.
      params._onchange = () => {
        const q = params.toString();
        this.search = q ? '?' + q : '';
        this._rebuild();
      };
      Object.defineProperty(this, 'searchParams', { value: params, configurable: true });
    }
    _rebuild() {
      const rebuilt = native.parse_url(
        `${this.protocol}//${this.host}${this.pathname}${this.search}${this.hash}`, null);
      if (rebuilt.ok) this.href = rebuilt.href;
    }
    toString() { return this.href; }
    toJSON() { return this.href; }
    static canParse(input, base) {
      return native.parse_url(String(input), base == null ? null : String(base)).ok;
    }
    static parse(input, base) {
      try { return new URL(input, base); } catch { return null; }
    }
    // Object URLs have no backing store here, but code that creates and
    // revokes them should not throw.
    static createObjectURL() { return 'blob:mar/0'; }
    static revokeObjectURL() {}
  }
  globalThis.URL = URL;

  // -- Intl ------------------------------------------------------------------

  // QuickJS is built without ICU, so there is no `Intl` at all. That is not a
  // cosmetic gap: a page that formats one date or one price through it throws a
  // ReferenceError inside its render, and an application that dies there paints
  // nothing. A whole page can be lost to a missing thousands separator.
  //
  // What follows covers the calls applications actually make, with English and
  // Russian tables and a neutral fallback for every other language. It is not a
  // locale database and does not pretend to be one: the goal is that formatting
  // returns something plausible instead of throwing, so the page renders and
  // the text around the number is there to read.
  (function () {
    if (typeof globalThis.Intl !== 'undefined') return;

    const MONTHS = {
      en: {
        long: ['January', 'February', 'March', 'April', 'May', 'June', 'July',
          'August', 'September', 'October', 'November', 'December'],
        short: ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'],
      },
      ru: {
        long: ['января', 'февраля', 'марта', 'апреля', 'мая', 'июня', 'июля',
          'августа', 'сентября', 'октября', 'ноября', 'декабря'],
        short: ['янв.', 'февр.', 'мар.', 'апр.', 'мая', 'июн.', 'июл.', 'авг.', 'сент.', 'окт.', 'нояб.', 'дек.'],
      },
    };
    const WEEKDAYS = {
      en: {
        long: ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'],
        short: ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'],
        narrow: ['S', 'M', 'T', 'W', 'T', 'F', 'S'],
      },
      ru: {
        long: ['воскресенье', 'понедельник', 'вторник', 'среда', 'четверг', 'пятница', 'суббота'],
        short: ['вс', 'пн', 'вт', 'ср', 'чт', 'пт', 'сб'],
        narrow: ['В', 'П', 'В', 'С', 'Ч', 'П', 'С'],
      },
    };
    const CURRENCY = {
      USD: '$', EUR: '€', RUB: '₽', GBP: '£', JPY: '¥', CNY: '¥',
      KRW: '₩', UAH: '₴', KZT: '₸', INR: '₹', TRY: '₺', BRL: 'R$',
      PLN: 'zł', CHF: 'CHF', SEK: 'kr', NOK: 'kr', DKK: 'kr', CAD: 'CA$', AUD: 'A$', BYN: 'Br',
    };
    // Languages that write the decimal with a comma, and put the day first.
    // They split into two camps on the group separator: a dot, or a space.
    // Everything unlisted is formatted the English way.
    const DOT_GROUPED = new Set(['de', 'es', 'it', 'pt', 'nl', 'da', 'tr', 'id', 'el', 'ro', 'hr', 'sr', 'sl', 'is', 'vi']);
    const SPACE_GROUPED = new Set(['ru', 'fr', 'pl', 'uk', 'cs', 'sk', 'sv', 'fi', 'nb', 'no', 'hu', 'kk', 'be', 'bg', 'lv', 'lt', 'et']);
    const COMMA_DECIMAL = new Set([...DOT_GROUPED, ...SPACE_GROUPED]);
    const DAY_FIRST = new Set([...COMMA_DECIMAL, 'en-GB', 'en-AU', 'en-IN']);
    // The separator a browser actually emits for these locales is a no-break
    // space, and pages that re-parse their own output match on it.
    const NBSP = ' ';

    const canon = (loc) => {
      const s = Array.isArray(loc) ? loc[0] : loc;
      return typeof s === 'string' && s ? s.replace(/_/g, '-') : 'en-US';
    };
    const primary = (loc) => canon(loc).toLowerCase().split('-')[0];
    // The tables only exist for two languages; a third falls back to English
    // words with its own separators, which reads better than throwing.
    const table = (loc) => (MONTHS[primary(loc)] ? primary(loc) : 'en');
    const pad = (n, w = 2) => String(n).padStart(w, '0');

    class Locale {
      constructor(tag) {
        const parts = canon(tag).split('-');
        this.baseName = canon(tag);
        this.language = parts[0];
        this.script = parts.slice(1).find((p) => /^[A-Za-z]{4}$/.test(p));
        this.region = parts.slice(1).find((p) => /^([A-Za-z]{2}|\d{3})$/.test(p));
      }
      toString() { return this.baseName; }
      maximize() { return this; }
      minimize() { return this; }
    }

    // A formatter is described by the option bag it was built with, and both
    // `format` and `formatToParts` read the same list of pieces, so a caller
    // that walks the parts and one that takes the string always agree.
    class DateTimeFormat {
      constructor(locales, options) {
        this.locale = canon(locales);
        this.t = table(locales);
        this.o = Object.assign({}, options || {});
      }
      resolvedOptions() {
        return Object.assign(
          { locale: this.locale, calendar: 'gregory', numberingSystem: 'latn',
            timeZone: this.o.timeZone || 'UTC' },
          this.o,
        );
      }
      _fields() {
        const o = this.o;
        const f = {
          weekday: o.weekday, year: o.year, month: o.month, day: o.day,
          hour: o.hour, minute: o.minute, second: o.second,
        };
        const ds = o.dateStyle;
        if (ds === 'full') Object.assign(f, { weekday: 'long', year: 'numeric', month: 'long', day: 'numeric' });
        else if (ds === 'long') Object.assign(f, { year: 'numeric', month: 'long', day: 'numeric' });
        else if (ds === 'medium') Object.assign(f, { year: 'numeric', month: 'short', day: 'numeric' });
        else if (ds === 'short') Object.assign(f, { year: '2-digit', month: 'numeric', day: 'numeric' });
        if (o.timeStyle) {
          f.hour = 'numeric';
          f.minute = '2-digit';
          if (o.timeStyle !== 'short') f.second = '2-digit';
        }
        const anyDate = f.weekday || f.year || f.month || f.day;
        const anyTime = f.hour || f.minute || f.second;
        // No options at all means the plain numeric date, as in a browser.
        if (!anyDate && !anyTime) Object.assign(f, { year: 'numeric', month: 'numeric', day: 'numeric' });
        return f;
      }
      formatToParts(input) {
        const d = input === undefined ? new Date() : new Date(input);
        if (isNaN(d.getTime())) throw new RangeError('Invalid time value');
        const f = this._fields();
        const dayFirst = DAY_FIRST.has(this.t) || DAY_FIRST.has(this.locale);
        const dotted = this.t !== 'en';
        const out = [];
        const lit = (value) => { if (value) out.push({ type: 'literal', value }); };

        if (f.weekday) {
          const forms = WEEKDAYS[this.t];
          out.push({ type: 'weekday', value: (forms[f.weekday] || forms.long)[d.getDay()] });
          if (f.year || f.month || f.day) lit(', ');
        }

        const named = f.month === 'long' || f.month === 'short';
        const year = () => ({ type: 'year',
          value: f.year === '2-digit' ? pad(d.getFullYear() % 100) : String(d.getFullYear()) });
        const month = () => ({ type: 'month', value: named
          ? MONTHS[this.t][f.month][d.getMonth()]
          : (f.month === '2-digit' || dotted ? pad(d.getMonth() + 1) : String(d.getMonth() + 1)) });
        const day = () => ({ type: 'day',
          value: f.day === '2-digit' || (dotted && !named) ? pad(d.getDate()) : String(d.getDate()) });

        if (named) {
          // "1 September 2026" against "September 1, 2026".
          if (dayFirst) {
            if (f.day) { out.push(day()); lit(' '); }
            if (f.month) out.push(month());
            if (f.year) { lit(' '); out.push(year()); if (this.t === 'ru') lit(' г.'); }
          } else {
            if (f.month) out.push(month());
            if (f.day) { lit(' '); out.push(day()); }
            if (f.year) { lit(', '); out.push(year()); }
          }
        } else if (f.year || f.month || f.day) {
          const sep = dotted ? '.' : '/';
          const order = dayFirst
            ? [f.day && day, f.month && month, f.year && year]
            : [f.month && month, f.day && day, f.year && year];
          let first = true;
          for (const make of order) {
            if (!make) continue;
            if (!first) lit(sep);
            out.push(make());
            first = false;
          }
        }

        if (f.hour || f.minute || f.second) {
          if (out.length) lit(', ');
          const h24 = this.o.hourCycle === 'h23' || this.o.hourCycle === 'h24'
            || this.o.hour12 === false
            || (this.o.hour12 === undefined && this.o.hourCycle === undefined && this.t !== 'en');
          const h = h24 ? d.getHours() : (d.getHours() % 12 || 12);
          out.push({ type: 'hour', value: f.hour === '2-digit' || h24 ? pad(h) : String(h) });
          if (f.minute) { lit(':'); out.push({ type: 'minute', value: pad(d.getMinutes()) }); }
          if (f.second) { lit(':'); out.push({ type: 'second', value: pad(d.getSeconds()) }); }
          if (!h24) { lit(' '); out.push({ type: 'dayPeriod', value: d.getHours() < 12 ? 'AM' : 'PM' }); }
          if (this.o.timeZoneName) { lit(' '); out.push({ type: 'timeZoneName', value: this.o.timeZone || 'UTC' }); }
        }
        return out;
      }
      format(input) { return this.formatToParts(input).map((p) => p.value).join(''); }
      formatRange(a, b) { return `${this.format(a)} – ${this.format(b)}`; }
      formatRangeToParts(a, b) {
        return this.formatToParts(a)
          .concat([{ type: 'literal', value: ' – ' }], this.formatToParts(b));
      }
      static supportedLocalesOf(l) { return l === undefined ? [] : [].concat(l).map(String); }
    }

    class NumberFormat {
      constructor(locales, options) {
        this.locale = canon(locales);
        this.t = table(locales);
        this.o = Object.assign({}, options || {});
      }
      resolvedOptions() {
        return Object.assign(
          { locale: this.locale, numberingSystem: 'latn', style: 'decimal',
            minimumIntegerDigits: 1, useGrouping: 'auto' },
          this.o,
        );
      }
      formatToParts(input) {
        const o = this.o;
        const lang = primary(this.locale);
        const comma = COMMA_DECIMAL.has(lang);
        const groupSep = DOT_GROUPED.has(lang) ? '.' : comma ? NBSP : ',';
        const decSep = comma ? ',' : '.';
        const style = o.style || 'decimal';
        let v = Number(input);
        if (isNaN(v)) return [{ type: 'nan', value: 'NaN' }];
        if (!isFinite(v)) {
          return (v < 0 ? [{ type: 'minusSign', value: '-' }] : [])
            .concat([{ type: 'infinity', value: '∞' }]);
        }
        if (style === 'percent') v *= 100;

        // Compact notation goes first: it changes the value, and the default
        // precision that follows depends on the shortened number, not the
        // original one. Options are never written back — one formatter is
        // reused across a whole page.
        let compact = '';
        let maxFraction = o.maximumFractionDigits;
        if (o.notation === 'compact') {
          // Compact suffixes are words, so they come from the language
          // table rather than from which separator the locale uses.
          const units = this.t === 'ru'
            ? [[1e9, ' млрд'], [1e6, ' млн'], [1e3, ' тыс.']]
            : [[1e12, 'T'], [1e9, 'B'], [1e6, 'M'], [1e3, 'K']];
          for (const [scale, suffix] of units) {
            if (Math.abs(v) >= scale) { v /= scale; compact = suffix; break; }
          }
          if (maxFraction === undefined) maxFraction = Math.abs(v) >= 100 ? 0 : 1;
        }

        let minFraction = o.minimumFractionDigits;
        if (style === 'currency') {
          if (minFraction === undefined) minFraction = 2;
          if (maxFraction === undefined) maxFraction = Math.max(2, minFraction);
        } else {
          if (minFraction === undefined) minFraction = 0;
          if (maxFraction === undefined) maxFraction = Math.max(minFraction, 3);
        }
        if (o.maximumSignificantDigits) v = Number(v.toPrecision(o.maximumSignificantDigits));

        const negative = v < 0;
        v = Math.abs(v);
        // toFixed pads out to the maximum; the minimum is what has to survive
        // the trim, so a price stays "9.90" and a count stays "9".
        let text = v.toFixed(Math.max(0, Math.min(maxFraction, 100)));
        if (text.includes('.')) {
          const [whole, digits] = text.split('.');
          const kept = digits.replace(/0+$/, '').padEnd(minFraction, '0');
          text = kept ? `${whole}.${kept}` : whole;
        }
        let [integer, fraction] = text.split('.');
        if (o.minimumIntegerDigits) integer = integer.padStart(o.minimumIntegerDigits, '0');

        const parts = [];
        if (negative) parts.push({ type: 'minusSign', value: '-' });
        else if (o.signDisplay === 'always' || (o.signDisplay === 'exceptZero' && v !== 0)) {
          parts.push({ type: 'plusSign', value: '+' });
        }

        const grouping = o.useGrouping === false || o.useGrouping === 'false' || o.useGrouping === 'never';
        // Split into groups of three from the right, without inventing a
        // separator to split on: the separator is chosen per locale below.
        const chunks = grouping ? [integer] : (integer.match(/\d{1,3}(?=(\d{3})*$)/g) || [integer]);
        chunks.forEach((chunk, i) => {
          if (i) parts.push({ type: 'group', value: groupSep });
          parts.push({ type: 'integer', value: chunk });
        });
        if (fraction) {
          parts.push({ type: 'decimal', value: decSep });
          parts.push({ type: 'fraction', value: fraction });
        }
        if (compact) parts.push({ type: 'compact', value: compact });
        if (style === 'percent') parts.push({ type: 'percentSign', value: comma ? `${NBSP}%` : '%' });
        if (style === 'unit' && o.unit) parts.push({ type: 'unit', value: ` ${o.unit}` });
        if (style === 'currency') {
          const code = String(o.currency || 'USD').toUpperCase();
          const known = CURRENCY[code];
          const symbol = o.currencyDisplay === 'code' || !known ? code : known;
          // The symbol trails the number in most of the world and leads it in
          // English, and that is also where the space goes or does not.
          if (comma) parts.push({ type: 'currency', value: `${NBSP}${symbol}` });
          else parts.splice(negative ? 1 : 0, 0,
            { type: 'currency', value: known ? symbol : `${symbol}${NBSP}` });
        }
        return parts;
      }
      format(input) { return this.formatToParts(input).map((p) => p.value).join(''); }
      formatRange(a, b) { return `${this.format(a)}–${this.format(b)}`; }
      static supportedLocalesOf(l) { return l === undefined ? [] : [].concat(l).map(String); }
    }

    class PluralRules {
      constructor(locales, options) {
        this.locale = canon(locales);
        this.t = table(locales);
        this.type = (options && options.type) || 'cardinal';
      }
      resolvedOptions() {
        return { locale: this.locale, type: this.type,
          pluralCategories: this.t === 'ru' ? ['one', 'few', 'many', 'other'] : ['one', 'other'] };
      }
      select(input) {
        const n = Math.abs(Number(input));
        if (this.type === 'ordinal') {
          if (this.t !== 'en') return 'other';
          const ones = n % 10, tens = n % 100;
          if (ones === 1 && tens !== 11) return 'one';
          if (ones === 2 && tens !== 12) return 'two';
          if (ones === 3 && tens !== 13) return 'few';
          return 'other';
        }
        // Russian is the rule a page visibly gets wrong when it is missing:
        // one form for 1, another for 2 to 4, a third for everything else.
        if (this.t === 'ru') {
          if (!Number.isInteger(n)) return 'other';
          const ones = n % 10, tens = n % 100;
          if (ones === 1 && tens !== 11) return 'one';
          if (ones >= 2 && ones <= 4 && (tens < 12 || tens > 14)) return 'few';
          return 'many';
        }
        return n === 1 ? 'one' : 'other';
      }
      static supportedLocalesOf(l) { return l === undefined ? [] : [].concat(l).map(String); }
    }

    const RU_UNITS = {
      second: ['секунду', 'секунды', 'секунд'],
      minute: ['минуту', 'минуты', 'минут'],
      hour: ['час', 'часа', 'часов'],
      day: ['день', 'дня', 'дней'],
      week: ['неделю', 'недели', 'недель'],
      month: ['месяц', 'месяца', 'месяцев'],
      quarter: ['квартал', 'квартала', 'кварталов'],
      year: ['год', 'года', 'лет'],
    };

    class RelativeTimeFormat {
      constructor(locales, options) {
        this.locale = canon(locales);
        this.t = table(locales);
        this.o = Object.assign({ numeric: 'always', style: 'long' }, options || {});
        this.plural = new PluralRules(locales);
      }
      resolvedOptions() { return Object.assign({ locale: this.locale, numberingSystem: 'latn' }, this.o); }
      format(value, unit) {
        const v = Number(value);
        const u = String(unit).replace(/s$/, '');
        const ru = this.t === 'ru';
        if (this.o.numeric === 'auto') {
          if (u === 'day' && v === 0) return ru ? 'сегодня' : 'today';
          if (u === 'day' && v === 1) return ru ? 'завтра' : 'tomorrow';
          if (u === 'day' && v === -1) return ru ? 'вчера' : 'yesterday';
          if (v === 0) return ru ? 'сейчас' : (u === 'second' ? 'now' : `this ${u}`);
        }
        const n = Math.abs(v);
        if (ru) {
          const forms = RU_UNITS[u] || [u, u, u];
          const category = this.plural.select(n);
          const word = forms[category === 'one' ? 0 : category === 'few' ? 1 : 2];
          return v < 0 ? `${n} ${word} назад` : `через ${n} ${word}`;
        }
        const word = n === 1 ? u : `${u}s`;
        return v < 0 ? `${n} ${word} ago` : `in ${n} ${word}`;
      }
      formatToParts(v, u) { return [{ type: 'literal', value: this.format(v, u) }]; }
      static supportedLocalesOf(l) { return l === undefined ? [] : [].concat(l).map(String); }
    }

    class Collator {
      constructor(locales, options) {
        this.locale = canon(locales);
        this.o = Object.assign({ usage: 'sort', sensitivity: 'variant' }, options || {});
        this.compare = this.compare.bind(this);
      }
      resolvedOptions() { return Object.assign({ locale: this.locale }, this.o); }
      compare(a, b) {
        let x = String(a), y = String(b);
        if (this.o.sensitivity === 'base' || this.o.sensitivity === 'accent') {
          x = x.toLowerCase();
          y = y.toLowerCase();
        }
        if (this.o.numeric) {
          const nx = parseFloat(x), ny = parseFloat(y);
          if (!isNaN(nx) && !isNaN(ny) && nx !== ny) return nx < ny ? -1 : 1;
        }
        return x < y ? -1 : x > y ? 1 : 0;
      }
      static supportedLocalesOf(l) { return l === undefined ? [] : [].concat(l).map(String); }
    }

    class ListFormat {
      constructor(locales, options) {
        this.locale = canon(locales);
        this.t = table(locales);
        this.o = Object.assign({ type: 'conjunction', style: 'long' }, options || {});
      }
      resolvedOptions() { return Object.assign({ locale: this.locale }, this.o); }
      format(list) {
        const items = Array.from(list || [], String);
        if (items.length < 2) return items[0] ?? '';
        if (this.o.type === 'unit') return items.join(', ');
        const ru = this.t === 'ru';
        const word = this.o.type === 'disjunction'
          ? (ru ? 'или' : 'or')
          : (ru ? 'и' : 'and');
        if (items.length === 2) return `${items[0]} ${word} ${items[1]}`;
        const head = items.slice(0, -1).join(', ');
        const last = items[items.length - 1];
        return ru ? `${head} ${word} ${last}` : `${head}, ${word} ${last}`;
      }
      formatToParts(list) { return [{ type: 'literal', value: this.format(list) }]; }
      static supportedLocalesOf(l) { return l === undefined ? [] : [].concat(l).map(String); }
    }

    class DisplayNames {
      constructor(locales, options) {
        this.locale = canon(locales);
        this.o = Object.assign({ style: 'long', fallback: 'code' }, options || {});
      }
      resolvedOptions() { return Object.assign({ locale: this.locale }, this.o); }
      // Without a name table the code itself is the specified fallback, and it
      // reads acceptably: a language picker shows "de" rather than nothing.
      of(code) { return String(code); }
      static supportedLocalesOf(l) { return l === undefined ? [] : [].concat(l).map(String); }
    }

    class Segmenter {
      constructor(locales, options) {
        this.locale = canon(locales);
        this.o = Object.assign({ granularity: 'grapheme' }, options || {});
      }
      resolvedOptions() { return Object.assign({ locale: this.locale }, this.o); }
      segment(input) {
        const text = String(input);
        const granularity = this.o.granularity;
        const segments = [];
        let index = 0;
        const push = (segment, wordLike) => {
          segments.push({ segment, index, input: text,
            isWordLike: granularity === 'word' ? wordLike : undefined });
          index += segment.length;
        };
        if (granularity === 'grapheme') {
          // Iterating a string walks code points, which is closer to graphemes
          // than code units are and is as far as this goes without tables.
          for (const ch of text) push(ch);
        } else if (granularity === 'word') {
          for (const m of text.matchAll(/[\p{L}\p{N}_']+|\s+|[^\p{L}\p{N}_'\s]/gu)) {
            push(m[0], /[\p{L}\p{N}]/u.test(m[0]));
          }
        } else {
          for (const m of text.matchAll(/[^.!?]+[.!?]*\s*/g)) push(m[0]);
        }
        return {
          [Symbol.iterator]: () => segments[Symbol.iterator](),
          containing: (at) => segments.find((s) => at >= s.index && at < s.index + s.segment.length),
        };
      }
      static supportedLocalesOf(l) { return l === undefined ? [] : [].concat(l).map(String); }
    }

    // `Intl.DateTimeFormat().resolvedOptions().timeZone` is the one way to
    // ask a browser what time zone it is in, and it is written without `new`
    // everywhere. A class throws on that call, so each one is published behind
    // a plain function that constructs it either way.
    const callable = (Cls) => {
      const fn = function (...args) { return new Cls(...args); };
      define(fn, 'name', { value: Cls.name });
      fn.prototype = Cls.prototype;
      fn.supportedLocalesOf = Cls.supportedLocalesOf;
      return fn;
    };

    const Intl = {
      Collator: callable(Collator),
      DateTimeFormat: callable(DateTimeFormat),
      DisplayNames: callable(DisplayNames),
      ListFormat: callable(ListFormat),
      Locale: callable(Locale),
      NumberFormat: callable(NumberFormat),
      PluralRules: callable(PluralRules),
      RelativeTimeFormat: callable(RelativeTimeFormat),
      Segmenter: callable(Segmenter),
      getCanonicalLocales: (l) =>
        (l === undefined ? [] : [].concat(l).map((x) => String(x).replace(/_/g, '-'))),
      supportedValuesOf: (key) => ({
        timeZone: ['UTC'], currency: Object.keys(CURRENCY), calendar: ['gregory'],
        numberingSystem: ['latn'], collation: ['default'], unit: [],
      }[key] || []),
    };
    define(Intl, Symbol.toStringTag, { value: 'Intl' });
    define(globalThis, 'Intl', { value: Intl, writable: true, configurable: true });

    // The locale-aware methods on the built-ins are the other half of this. A
    // page that never names `Intl` still calls `toLocaleString` on a number,
    // and QuickJS's own version ignores the locale it was handed.
    method(Date.prototype, 'toLocaleDateString', function (l, o) {
      return new DateTimeFormat(l,
        Object.assign({ year: 'numeric', month: 'numeric', day: 'numeric' }, o)).format(this);
    });
    method(Date.prototype, 'toLocaleTimeString', function (l, o) {
      return new DateTimeFormat(l,
        Object.assign({ hour: 'numeric', minute: '2-digit', second: '2-digit' }, o)).format(this);
    });
    method(Date.prototype, 'toLocaleString', function (l, o) {
      return new DateTimeFormat(l, Object.assign(
        { year: 'numeric', month: 'numeric', day: 'numeric',
          hour: 'numeric', minute: '2-digit', second: '2-digit' }, o)).format(this);
    });
    method(Number.prototype, 'toLocaleString', function (l, o) {
      return new NumberFormat(l, o).format(this.valueOf());
    });
    if (typeof BigInt !== 'undefined') {
      method(BigInt.prototype, 'toLocaleString', function (l, o) {
        return new NumberFormat(l, o).format(Number(this));
      });
    }
  })();

  // -- misc web APIs -------------------------------------------------------

  globalThis.FormData = class FormData {
    constructor() { this._e = []; }
    append(k, v) { this._e.push([String(k), v]); }
    set(k, v) { this.delete(k); this.append(k, v); }
    get(k) { const f = this._e.find((e) => e[0] === String(k)); return f ? f[1] : null; }
    getAll(k) { return this._e.filter((e) => e[0] === String(k)).map((e) => e[1]); }
    has(k) { return this._e.some((e) => e[0] === String(k)); }
    delete(k) { this._e = this._e.filter((e) => e[0] !== String(k)); }
    forEach(fn, t) { for (const [k, v] of this._e) fn.call(t, v, k, this); }
    entries() { return this._e[Symbol.iterator](); }
    keys() { return this._e.map((e) => e[0])[Symbol.iterator](); }
    values() { return this._e.map((e) => e[1])[Symbol.iterator](); }
    [Symbol.iterator]() { return this.entries(); }
    toString() { return new URLSearchParams(this._e).toString(); }
  };

  globalThis.DOMParser = class DOMParser {
    parseFromString(html) {
      // Parse into a detached subtree of the live document. Callers read it
      // with the same node API as the page itself.
      const holder = native.create_element('html');
      holder.innerHTML = String(html);
      return holder;
    }
  };
  globalThis.XMLSerializer = class XMLSerializer {
    serializeToString(node) { return node.outerHTML ?? ''; }
  };

  globalThis.Blob = class Blob {
    constructor(parts = [], options = {}) {
      this._t = Array.from(parts, (p) => (typeof p === 'string' ? p : new TextDecoder().decode(p))).join('');
      this.size = this._t.length;
      this.type = options.type || '';
    }
    text() { return Promise.resolve(this._t); }
    slice(start, end, type) { return new Blob([this._t.slice(start, end)], { type: type ?? this.type }); }
    bytes() { return Promise.resolve(new TextEncoder().encode(this._t)); }
    arrayBuffer() { return this.bytes().then((b) => b.buffer); }
    stream() {
      const text = this._t;
      return new ReadableStream({
        start(controller) {
          if (text) controller.enqueue(new TextEncoder().encode(text));
          controller.close();
        },
      });
    }
  };
  globalThis.File = class File extends globalThis.Blob {
    constructor(parts, name) { super(parts); this.name = name; this.lastModified = 0; }
  };
  globalThis.FileReader = class FileReader {
    readAsText(blob) {
      blob.text().then((t) => {
        this.result = t;
        if (this.onload) this.onload({ target: this });
      });
    }
    addEventListener() {} removeEventListener() {}
  };

  globalThis.structuredClone = globalThis.structuredClone ||
    ((v) => JSON.parse(JSON.stringify(v)));

  const b64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
  globalThis.btoa = globalThis.btoa || function (input) {
    const s = String(input);
    let out = '';
    for (let i = 0; i < s.length; i += 3) {
      const a = s.charCodeAt(i), b = s.charCodeAt(i + 1), c = s.charCodeAt(i + 2);
      const n = (a << 16) | ((b || 0) << 8) | (c || 0);
      out += b64[(n >> 18) & 63] + b64[(n >> 12) & 63] +
        (isNaN(b) ? '=' : b64[(n >> 6) & 63]) + (isNaN(c) ? '=' : b64[n & 63]);
    }
    return out;
  };
  globalThis.atob = globalThis.atob || function (input) {
    const s = String(input).replace(/=+$/, '');
    let out = '', bits = 0, acc = 0;
    for (const ch of s) {
      const v = b64.indexOf(ch);
      if (v < 0) continue;
      acc = (acc << 6) | v;
      bits += 6;
      if (bits >= 8) { bits -= 8; out += String.fromCharCode((acc >> bits) & 0xff); }
    }
    return out;
  };

  globalThis.crypto = globalThis.crypto || {
    // Deterministic, not secure. Pages use this for cache-busting ids far more
    // often than for anything that matters, and determinism aids debugging.
    getRandomValues(arr) {
      for (let i = 0; i < arr.length; i++) arr[i] = (i * 2654435761) % 4294967296;
      return arr;
    },
    randomUUID() {
      let i = 0;
      return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, (c) => {
        const r = (i++ * 7 + 5) % 16;
        return (c === 'x' ? r : (r & 3) | 8).toString(16);
      });
    },
    subtle: {},
  };

  globalThis.CSS = { supports: () => false, escape: (s) => String(s).replace(/[^\w-]/g, '\\$&') };
  globalThis.customElements = {
    define() {}, get: () => undefined,
    whenDefined: () => Promise.resolve(), upgrade() {},
  };

  // -- text, bytes and streams ----------------------------------------------

  // A bundle that decodes anything — a wasm payload, a server-sent stream, a
  // response read progressively — reaches for these, and a missing constructor
  // is a ReferenceError that stops the script that wanted it. UTF-8 is done
  // here rather than natively because it is twenty lines and the alternative
  // is a bridge call per chunk.
  globalThis.TextEncoder = class TextEncoder {
    get encoding() { return 'utf-8'; }
    encode(input = '') {
      const s = String(input);
      const out = [];
      for (let i = 0; i < s.length; i++) {
        let c = s.codePointAt(i);
        if (c > 0xffff) i++;
        if (c < 0x80) out.push(c);
        else if (c < 0x800) out.push(0xc0 | (c >> 6), 0x80 | (c & 63));
        else if (c < 0x10000) out.push(0xe0 | (c >> 12), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
        else out.push(0xf0 | (c >> 18), 0x80 | ((c >> 12) & 63), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
      }
      return new Uint8Array(out);
    }
    encodeInto(input, target) {
      const bytes = this.encode(input);
      const written = Math.min(bytes.length, target.length);
      target.set(bytes.subarray(0, written));
      return { read: input.length, written };
    }
  };

  globalThis.TextDecoder = class TextDecoder {
    constructor(label = 'utf-8', options = {}) {
      this.encoding = String(label).toLowerCase();
      this.fatal = !!options.fatal;
      this.ignoreBOM = !!options.ignoreBOM;
    }
    decode(input) {
      if (input == null) return '';
      const bytes = input instanceof Uint8Array ? input
        : ArrayBuffer.isView(input) ? new Uint8Array(input.buffer, input.byteOffset, input.byteLength)
        : new Uint8Array(input);
      // Anything single-byte is decoded as latin1; everything else as UTF-8,
      // which is what the label says in all but a rounding error of pages.
      if (this.encoding === 'windows-1252' || this.encoding === 'latin1' || this.encoding === 'iso-8859-1') {
        let out = '';
        for (const b of bytes) out += String.fromCharCode(b);
        return out;
      }
      let out = '';
      for (let i = 0; i < bytes.length;) {
        const b = bytes[i];
        let cp, size;
        if (b < 0x80) { cp = b; size = 1; }
        else if ((b & 0xe0) === 0xc0) { cp = b & 31; size = 2; }
        else if ((b & 0xf0) === 0xe0) { cp = b & 15; size = 3; }
        else if ((b & 0xf8) === 0xf0) { cp = b & 7; size = 4; }
        else { out += '�'; i++; continue; }
        if (i + size > bytes.length) { out += '�'; break; }
        for (let k = 1; k < size; k++) cp = (cp << 6) | (bytes[i + k] & 63);
        out += String.fromCodePoint(cp);
        i += size;
      }
      // A byte-order mark is metadata, not text, and leaving it in front of a
      // JSON document is enough to make `JSON.parse` throw.
      if (!this.ignoreBOM && out.charCodeAt(0) === 0xfeff) out = out.slice(1);
      return out;
    }
  };

  // Streams, as far as reading goes. Everything the engine produces is already
  // in memory, so a stream here is a queue with a promise-shaped door on it:
  // `for await (const chunk of response.body)` works, backpressure does not
  // exist, and neither costs a page anything it would notice.
  class ReadableStream {
    constructor(source = {}, _strategy) {
      this._chunks = [];
      this._closed = false;
      this._error = null;
      this._waiting = [];
      this._source = source;
      this.locked = false;
      const controller = {
        enqueue: (chunk) => { this._chunks.push(chunk); this._wake(); },
        close: () => { this._closed = true; this._wake(); },
        error: (e) => { this._error = e; this._closed = true; this._wake(); },
        get desiredSize() { return 1; },
      };
      this._controller = controller;
      try {
        if (typeof source.start === 'function') source.start(controller);
      } catch (e) {
        this._error = e;
        this._closed = true;
      }
    }
    _wake() {
      const waiting = this._waiting;
      this._waiting = [];
      for (const resolve of waiting) resolve();
    }
    async _pull() {
      while (!this._chunks.length && !this._closed) {
        if (typeof this._source.pull === 'function') {
          await this._source.pull(this._controller);
          if (this._chunks.length || this._closed) break;
        }
        await new Promise((resolve) => this._waiting.push(resolve));
      }
      if (this._chunks.length) return { value: this._chunks.shift(), done: false };
      if (this._error) throw this._error;
      return { value: undefined, done: true };
    }
    getReader() {
      this.locked = true;
      const stream = this;
      return {
        read: () => stream._pull(),
        cancel: (reason) => stream.cancel(reason),
        releaseLock() { stream.locked = false; },
        get closed() { return Promise.resolve(); },
      };
    }
    cancel(reason) {
      this._chunks.length = 0;
      this._closed = true;
      this._wake();
      try {
        if (typeof this._source.cancel === 'function') this._source.cancel(reason);
      } catch (e) { /* a cancel that throws still cancels */ }
      return Promise.resolve();
    }
    pipeThrough(pair) { return pair.readable; }
    pipeTo() { return Promise.resolve(); }
    tee() { return [this, this]; }
    [Symbol.asyncIterator]() {
      const reader = this.getReader();
      return {
        next: () => reader.read(),
        return: () => { reader.releaseLock(); return Promise.resolve({ done: true }); },
        [Symbol.asyncIterator]() { return this; },
      };
    }
  }
  globalThis.ReadableStream = ReadableStream;
  globalThis.WritableStream = class WritableStream {
    constructor(sink = {}) { this._sink = sink; this.locked = false; }
    getWriter() {
      const sink = this._sink;
      this.locked = true;
      return {
        write: (chunk) => Promise.resolve(sink.write && sink.write(chunk)),
        close: () => Promise.resolve(sink.close && sink.close()),
        abort: () => Promise.resolve(),
        releaseLock: () => { this.locked = false; },
        get ready() { return Promise.resolve(); },
        get closed() { return Promise.resolve(); },
      };
    }
  };
  globalThis.TransformStream = class TransformStream {
    constructor() {
      this.readable = new ReadableStream();
      this.writable = new globalThis.WritableStream({
        write: (chunk) => this.readable._controller.enqueue(chunk),
        close: () => this.readable._controller.close(),
      });
    }
  };
  // The stream-shaped codecs, for a page that decodes a response chunk by
  // chunk. Everything here arrives in one piece, so the "stream" is a pair of
  // ends with a single chunk between them.
  globalThis.TextEncoderStream = class TextEncoderStream {
    constructor() {
      const encoder = new TextEncoder();
      this._t = new TransformStream();
      this.writable = new WritableStream({
        write: (chunk) => this._t.readable._controller.enqueue(encoder.encode(String(chunk))),
        close: () => this._t.readable._controller.close(),
      });
      this.readable = this._t.readable;
      this.encoding = 'utf-8';
    }
  };
  globalThis.TextDecoderStream = class TextDecoderStream {
    constructor(label) {
      const decoder = new TextDecoder(label);
      this._t = new TransformStream();
      this.writable = new WritableStream({
        write: (chunk) => this._t.readable._controller.enqueue(decoder.decode(chunk)),
        close: () => this._t.readable._controller.close(),
      });
      this.readable = this._t.readable;
      this.encoding = decoder.encoding;
    }
  };
  globalThis.CompressionStream = class CompressionStream { constructor() { throw new TypeError('CompressionStream is not supported'); } };
  globalThis.DecompressionStream = class DecompressionStream { constructor() { throw new TypeError('DecompressionStream is not supported'); } };

  globalThis.ByteLengthQueuingStrategy = class ByteLengthQueuingStrategy {
    constructor(o = {}) { this.highWaterMark = o.highWaterMark ?? 1; }
    size(chunk) { return chunk?.byteLength ?? 0; }
  };
  globalThis.CountQueuingStrategy = class CountQueuingStrategy {
    constructor(o = {}) { this.highWaterMark = o.highWaterMark ?? 1; }
    size() { return 1; }
  };

  // -- engine repairs --------------------------------------------------------

  // `Iterator.prototype.find` in this build of QuickJS never releases an item
  // the predicate rejected, so every element before the match is leaked. A
  // leaked object is still referenced when the runtime is torn down, which is
  // a hard abort — one page using this one method takes down a whole batch.
  // It is a small method and correct is easier than clever: `for...of` closes
  // the iterator on the way out, which is what the spec asks for.
  (function () {
    const proto = Object.getPrototypeOf(Object.getPrototypeOf([][Symbol.iterator]()));
    if (!proto || typeof proto.find !== 'function') return;
    method(proto, 'find', function find(predicate) {
      if (typeof predicate !== 'function') {
        throw new TypeError('Iterator.prototype.find: predicate is not a function');
      }
      let index = 0;
      for (const value of this) {
        if (predicate(value, index++)) return value;
      }
      return undefined;
    });
  })();

  // -- platform odds and ends ------------------------------------------------

  // Thrown by name from more DOM code than one would expect, and caught by
  // name too: `e instanceof DOMException` on an undefined global is a
  // TypeError raised inside a catch block, which is the worst place for one.
  globalThis.DOMException = class DOMException extends Error {
    constructor(message = '', name = 'Error') {
      super(message);
      this.name = name;
      this.code = 0;
    }
  };

  // `localStorage instanceof Storage` is how a page tells a real storage area
  // from a stub someone else installed.
  globalThis.Storage = class Storage {};
  try {
    Object.setPrototypeOf(localStorage, globalThis.Storage.prototype);
    Object.setPrototypeOf(sessionStorage, globalThis.Storage.prototype);
  } catch (e) { /* a frozen storage object keeps its own prototype */ }

  // Style sheets are constructed by design systems that adopt them into a
  // shadow root, which nothing here has; the object exists so the call that
  // makes one does not throw before the fallback path runs.
  globalThis.CSSStyleSheet = class CSSStyleSheet {
    constructor() { this.cssRules = []; this.rules = this.cssRules; this.disabled = false; }
    insertRule(rule, index = this.cssRules.length) {
      this.cssRules.splice(index, 0, { cssText: String(rule) });
      return index;
    }
    deleteRule(index) { this.cssRules.splice(index, 1); }
    replace(text) { this.replaceSync(text); return Promise.resolve(this); }
    replaceSync(text) { this.cssRules = [{ cssText: String(text) }]; this.rules = this.cssRules; }
  };
  globalThis.StyleSheet = globalThis.CSSStyleSheet;
  globalThis.CSSRule = class CSSRule {};
  globalThis.MediaQueryList = class MediaQueryList {};

  // An abort signal that actually fires. A fetch here is synchronous and can
  // never be interrupted, but the listener side is used for cleanup — a
  // component that unmounts mid-request removes its own work on this event,
  // and without it the work is left behind.
  class AbortSignal {
    constructor() {
      this.aborted = false;
      this.reason = undefined;
      this.onabort = null;
      this._listeners = [];
    }
    addEventListener(type, fn) { if (type === 'abort' && fn) this._listeners.push(fn); }
    removeEventListener(type, fn) {
      if (type === 'abort') this._listeners = this._listeners.filter((f) => f !== fn);
    }
    dispatchEvent() { return true; }
    throwIfAborted() { if (this.aborted) throw this.reason; }
    _abort(reason) {
      if (this.aborted) return;
      this.aborted = true;
      this.reason = reason ?? new globalThis.DOMException('signal is aborted', 'AbortError');
      const event = { type: 'abort', target: this, currentTarget: this };
      for (const fn of this._listeners.slice()) {
        try { (fn.handleEvent || fn).call(this, event); }
        catch (e) { native.record_error('abort listener', String((e && e.stack) || e)); }
      }
      if (typeof this.onabort === 'function') {
        try { this.onabort(event); }
        catch (e) { native.record_error('onabort', String((e && e.stack) || e)); }
      }
    }
    static abort(reason) { const s = new AbortSignal(); s._abort(reason); return s; }
    static timeout() { return new AbortSignal(); }
    static any(signals) {
      const merged = new AbortSignal();
      for (const s of signals || []) {
        if (s?.aborted) { merged._abort(s.reason); break; }
        s?.addEventListener('abort', () => merged._abort(s.reason));
      }
      return merged;
    }
  }
  globalThis.AbortSignal = AbortSignal;
  globalThis.AbortController = class AbortController {
    constructor() { this.signal = new AbortSignal(); }
    abort(reason) { this.signal._abort(reason); }
  };

  // -- a synthetic spatial index -------------------------------------------

  // A CDP client does not click a selector. It reads an element's rectangle,
  // takes the centre point, and sends those coordinates back as a mouse event.
  // Without layout every rectangle is the same zero-sized box at the origin, so
  // every centre is (0, 0) and a click can never say which element it meant.
  //
  // So elements are handed tiles from an imaginary grid, one each, on first
  // request: unique, non-zero, and remembered, so a coordinate maps back to the
  // element it came from. This is a coordinate registry and nothing more — the
  // tiles carry no information about where anything really sits on a page, and
  // they are handed out in the order elements are asked about rather than in
  // document order.
  //
  // Only a client measuring gets one. The page's own scripts keep seeing the
  // zeros they see everywhere else, so a page that branches on
  // `rect.width === 0` behaves exactly as it does with no client attached.
  const spatial = (() => {
    const TILE_W = 100;
    const TILE_H = 20;
    // How many client measurements are in progress. Nothing is measured
    // outside one, so the page's own scripts never see a box.
    let asking = 0;
    let next = 0;
    const index = new Map();
    const nodes = new Map();
    const columns = () => Math.max(1, Math.floor(innerWidth / TILE_W));
    const tile = (i) => {
      const left = (i % columns()) * TILE_W;
      const top = Math.floor(i / columns()) * TILE_H;
      return {
        x: left, y: top, left, top,
        right: left + TILE_W, bottom: top + TILE_H,
        width: TILE_W, height: TILE_H,
        toJSON() { return { ...this }; },
      };
    };
    return {
      get enabled() { return asking > 0; },
      // Measurement is counted rather than toggled, so one client asking
      // inside another's question cannot end it early.
      open() { asking += 1; },
      close() { asking = Math.max(0, asking - 1); },
      measuring(f) {
        asking += 1;
        try {
          return f();
        } finally {
          asking -= 1;
        }
      },
      rect(node) {
        if (asking === 0 || !node) return zeroRect();
        const key = keyOf(node);
        let i = index.get(key);
        if (i === undefined) {
          i = next++;
          index.set(key, i);
          nodes.set(i, node);
        }
        return tile(i);
      },
      at(x, y) {
        const cols = columns();
        const col = Math.floor(x / TILE_W);
        const row = Math.floor(y / TILE_H);
        if (col < 0 || col >= cols || row < 0) return null;
        return nodes.get(row * cols + col) ?? null;
      },
      // How much of the imaginary viewport is in use. A client clamps a click
      // point to the layout viewport it was told about and drops it if that
      // leaves no area, so the reported size has to cover the tiles handed out.
      extent() {
        return [columns() * TILE_W, Math.max(1, Math.ceil(next / columns())) * TILE_H];
      },
    };
  })();

  /// Bracket a client's own evaluation, which is the only thing that measures.
  globalThis.__mar_layout_client = function (on) {
    if (on) spatial.open();
    else spatial.close();
    return spatial.enabled;
  };

  globalThis.__mar_layout_extent = function () {
    return spatial.extent();
  };

  /// The tile assigned to a node, assigning one if this is the first ask.
  globalThis.__mar_layout_rect = function (id) {
    const node = native.node_by_id(id);
    if (!node) return null;
    const r = spatial.measuring(() => spatial.rect(node));
    return { x: r.x, y: r.y, width: r.width, height: r.height };
  };

  // -- synthetic input -----------------------------------------------------

  const MODIFIER = { alt: 1, ctrl: 2, meta: 4, shift: 8 };
  const modifierInit = (mask) => ({
    altKey: !!(mask & MODIFIER.alt),
    ctrlKey: !!(mask & MODIFIER.ctrl),
    metaKey: !!(mask & MODIFIER.meta),
    shiftKey: !!(mask & MODIFIER.shift),
  });

  /// Dispatch a mouse event at a point, on whichever element owns that tile.
  ///
  /// Returns the element's node id, or 0 when the point belongs to no element.
  globalThis.__mar_input_mouse = function (type, x, y, button, clickCount, modifiers) {
    const target = spatial.at(x, y);
    if (!target) return 0;
    const buttons = { left: 1, right: 2, middle: 4 };
    const init = {
      bubbles: true, cancelable: true, composed: true,
      clientX: x, clientY: y, screenX: x, screenY: y,
      detail: clickCount || 0,
      button: button === 'right' ? 2 : button === 'middle' ? 1 : 0,
      buttons: buttons[button] ?? 0,
      ...modifierInit(modifiers),
    };
    const fire = (name) =>
      dispatchEvent.call(target, new MouseEvent(name, init));

    if (type === 'mouseMoved') {
      fire('mousemove');
    } else if (type === 'mousePressed') {
      if (typeof target.focus === 'function') target.focus();
      fire('mousedown');
    } else if (type === 'mouseReleased') {
      fire('mouseup');
      if (init.button === 0) {
        // A browser runs the element's activation behaviour before the click
        // event reaches any listener, so a handler reading `checked` sees the
        // new state. Following a link is not part of it: there is nothing to
        // navigate to from here.
        if (target.tagName === 'INPUT') {
          const kind = (target.getAttribute('type') || '').toLowerCase();
          if (kind === 'checkbox') target.checked = !target.checked;
          else if (kind === 'radio') target.checked = true;
        }
        fire('click');
      }
    } else if (type === 'mouseWheel') {
      fire('wheel');
    }
    return target.marNodeId ?? 0;
  };

  /// Dispatch a key event at the focused element, editing it when it takes text.
  globalThis.__mar_input_key = function (type, key, code, text, modifiers) {
    const target = document.activeElement;
    if (!target) return 0;
    const init = {
      bubbles: true, cancelable: true, composed: true,
      key: String(key || ''), code: String(code || ''),
      keyCode: (key && key.length === 1 ? key.toUpperCase() : '').charCodeAt(0) || 0,
      ...modifierInit(modifiers),
    };
    const editable = target.tagName === 'INPUT' || target.tagName === 'TEXTAREA';

    const insert = () => {
      if (!editable) return;
      let value = String(target.value ?? '');
      if (init.key === 'Backspace') value = value.slice(0, -1);
      else if (init.key === 'Enter') {
        if (target.tagName !== 'TEXTAREA') {
          // Enter in a single-line field submits the form. Nothing navigates
          // here, but the page's own submit handler is what a caller wanted.
          dispatchEvent.call(target, new Event('change', { bubbles: true }));
          const form = target.closest('form');
          if (form) {
            dispatchEvent.call(form, new Event('submit', { bubbles: true, cancelable: true }));
          }
          return;
        }
        value += '\n';
      } else if (text) value += String(text);
      else return;
      target.value = value;
      dispatchEvent.call(target, new InputEvent('input', { bubbles: true, data: text || null }));
    };

    if (type === 'keyDown' || type === 'rawKeyDown') {
      const proceed = dispatchEvent.call(target, new KeyboardEvent('keydown', init));
      if (!proceed) return target.marNodeId ?? 0;
      if (text) dispatchEvent.call(target, new KeyboardEvent('keypress', init));
      insert();
    } else if (type === 'char') {
      dispatchEvent.call(target, new KeyboardEvent('keypress', init));
      insert();
    } else if (type === 'insertText') {
      // Text put in without pretending any key was pressed.
      insert();
    } else if (type === 'keyUp') {
      dispatchEvent.call(target, new KeyboardEvent('keyup', init));
    }
    return target.marNodeId ?? 0;
  };

  // -- remote object handles -----------------------------------------------

  // A CDP client refers to a live JS value across separate protocol messages
  // by an opaque id. Keeping the values in JavaScript rather than pinning them
  // from Rust means the engine needs no cross-language lifetime tracking, and
  // releasing a handle is a map delete.
  const handles = new Map();
  let nextHandle = 1;

  // Exposed for the CDP layer, which addresses nodes by arena id.
  globalThis.__mar_node_by_id = function (id) {
    return native.node_by_id(id);
  };

  globalThis.__mar_handle_put = function (value) {
    const id = nextHandle++;
    handles.set(id, value);
    return id;
  };

  globalThis.__mar_handle_get = function (id) {
    return handles.get(id);
  };

  /// Park a value, awaiting it first when it is thenable.
  ///
  /// CDP's `awaitPromise` means the server settles the promise before
  /// describing the result. A client's async helpers — the iterator `$$` walks,
  /// every `await` inside an evaluated function — return promises, so without
  /// this the client is handed a Promise object and finds nothing in it.
  globalThis.__mar_await = function (value) {
    const slot = { state: 'pending', value: undefined };
    if (value && (typeof value === 'object' || typeof value === 'function')
        && typeof value.then === 'function') {
      value.then(
        (v) => { slot.state = 'fulfilled'; slot.value = v; },
        (e) => { slot.state = 'rejected'; slot.value = e; },
      );
    } else {
      slot.state = 'fulfilled';
      slot.value = value;
    }
    return __mar_handle_put(slot);
  };

  /// Read a parked slot: its state, and its value once settled.
  globalThis.__mar_settled = function (id, byValue) {
    const slot = handles.get(id);
    if (!slot) return JSON.stringify({ state: 'missing' });
    if (slot.state === 'pending') return JSON.stringify({ state: 'pending' });
    return JSON.stringify({
      state: slot.state,
      result: __mar_describe(slot.value, byValue),
    });
  };

  /// Enumerate a handle's own properties as CDP PropertyDescriptors.
  ///
  /// A client walks an array result this way: it gets one handle for the array
  /// and then asks for the elements. Returning nothing here is why `$$` and
  /// `$$eval` come back empty even though the query itself matched.
  globalThis.__mar_handle_properties = function (id, ownOnly) {
    const value = handles.get(id);
    if (value === undefined || value === null) return [];
    const out = [];
    const push = (name, v, enumerable) => {
      out.push({
        name: String(name),
        value: __mar_describe(v, false),
        writable: true,
        configurable: true,
        enumerable,
        isOwn: true,
      });
    };
    if (Array.isArray(value)) {
      for (let i = 0; i < value.length; i++) push(i, value[i], true);
      push('length', value.length, false);
      return out;
    }
    if (typeof value === 'object' || typeof value === 'function') {
      for (const key of Object.getOwnPropertyNames(value)) {
        // A getter can throw or have side effects; skip anything that is not
        // a plain data property.
        const descriptor = Object.getOwnPropertyDescriptor(value, key);
        if (!descriptor || !('value' in descriptor)) continue;
        push(key, descriptor.value, descriptor.enumerable !== false);
      }
    }
    return out;
  };

  globalThis.__mar_handle_release = function (id) {
    handles.delete(id);
  };

  globalThis.__mar_handle_clear = function () {
    handles.clear();
    nextHandle = 1;
  };

  /// Describe a value the way CDP's RemoteObject does, keeping a handle when
  /// the value cannot be sent by value.
  globalThis.__mar_describe = function (value, byValue) {
    const type = typeof value;
    if (value === null) return { type: 'object', subtype: 'null', value: null };
    if (type === 'undefined') return { type: 'undefined' };
    if (type === 'boolean' || type === 'number' || type === 'string') {
      return { type, value };
    }
    if (type === 'bigint') return { type: 'bigint', value: String(value) };
    if (type === 'symbol') return { type: 'symbol', description: String(value) };
    if (type === 'function') {
      return {
        type: 'function',
        className: 'Function',
        description: value.name ? `function ${value.name}() {}` : 'function () {}',
        objectId: String(__mar_handle_put(value)),
      };
    }
    if (value instanceof Error) {
      return {
        type: 'object', subtype: 'error',
        className: value.name || 'Error',
        description: `${value.name}: ${value.message}`,
      };
    }
    if (value instanceof Node) {
      return {
        type: 'object', subtype: 'node',
        className: value.nodeName,
        description: value.nodeName,
        objectId: String(__mar_handle_put(value)),
      };
    }
    if (byValue) {
      // The client asked for the value itself; anything JSON can carry goes
      // across, and anything it cannot becomes a handle instead.
      try {
        return { type: 'object', value: JSON.parse(JSON.stringify(value)) };
      } catch {
        /* fall through to a handle */
      }
    }
    const isArray = Array.isArray(value);
    return {
      type: 'object',
      subtype: isArray ? 'array' : undefined,
      className: isArray ? 'Array' : (value.constructor && value.constructor.name) || 'Object',
      description: isArray ? `Array(${value.length})` : 'Object',
      objectId: String(__mar_handle_put(value)),
    };
  };

  // -- the rest of the DOM's names ------------------------------------------

  // A page does not only call these; it tests against them. `x instanceof
  // NodeList`, `e instanceof DragEvent`, `new MessageChannel()` — a missing
  // name is a ReferenceError or a TypeError, and one of those inside a
  // framework's own code takes the render with it. Most of what follows is a
  // name and a shape, because a name and a shape is what the page wants.
  (function () {
    const named = (name, ctor) => {
      if (name in globalThis) return globalThis[name];
      define(ctor, 'name', { value: name });
      define(globalThis, name, { value: ctor, writable: true, configurable: true });
      return ctor;
    };

    // Collections. `querySelectorAll` hands back a real array here, so these
    // answer for one rather than pretending to be a separate species.
    const isArrayLike = (v) => !!v && typeof v === 'object' && typeof v.length === 'number';
    for (const name of ['NodeList', 'HTMLCollection', 'HTMLAllCollection',
      'HTMLFormControlsCollection', 'HTMLOptionsCollection', 'RadioNodeList',
      'FileList', 'DOMStringList', 'StyleSheetList', 'NamedNodeMap']) {
      const ctor = function () { throw new TypeError(`Illegal constructor: ${name}`); };
      define(ctor, Symbol.hasInstance, { value: isArrayLike });
      named(name, ctor);
    }

    // The singletons the page already has, given the constructor its
    // `instanceof` looks for.
    for (const [name, instance] of [
      ['Window', globalThis], ['Navigator', globalThis.navigator],
      ['Location', globalThis.location], ['History', globalThis.history],
      ['Screen', globalThis.screen], ['Performance', globalThis.performance],
      ['Console', globalThis.console], ['CustomElementRegistry', globalThis.customElements],
      ['Crypto', globalThis.crypto], ['DOMImplementation', document.implementation],
    ]) {
      if (!instance) continue;
      const ctor = function () { throw new TypeError(`Illegal constructor: ${name}`); };
      define(ctor, Symbol.hasInstance, { value: (v) => v === instance });
      named(name, ctor);
    }
    if (globalThis.crypto) named('SubtleCrypto', class SubtleCrypto {});

    // Geometry. Constructible, because a page that lays anything out builds
    // these itself as often as it reads them off an element.
    class DOMRectReadOnly {
      constructor(x = 0, y = 0, width = 0, height = 0) {
        define(this, 'x', { value: +x, enumerable: true });
        define(this, 'y', { value: +y, enumerable: true });
        define(this, 'width', { value: +width, enumerable: true });
        define(this, 'height', { value: +height, enumerable: true });
      }
      get top() { return Math.min(this.y, this.y + this.height); }
      get bottom() { return Math.max(this.y, this.y + this.height); }
      get left() { return Math.min(this.x, this.x + this.width); }
      get right() { return Math.max(this.x, this.x + this.width); }
      toJSON() {
        const { x, y, width, height, top, right, bottom, left } = this;
        return { x, y, width, height, top, right, bottom, left };
      }
      static fromRect(r = {}) { return new DOMRectReadOnly(r.x, r.y, r.width, r.height); }
    }
    named('DOMRectReadOnly', DOMRectReadOnly);
    named('DOMRect', class DOMRect extends DOMRectReadOnly {
      constructor(x, y, width, height) {
        super(x, y, width, height);
        for (const k of ['x', 'y', 'width', 'height']) {
          define(this, k, { value: this[k], writable: true, enumerable: true });
        }
      }
      static fromRect(r = {}) { return new DOMRect(r.x, r.y, r.width, r.height); }
    });
    class DOMPointReadOnly {
      constructor(x = 0, y = 0, z = 0, w = 1) {
        this.x = +x; this.y = +y; this.z = +z; this.w = +w;
      }
      toJSON() { return { x: this.x, y: this.y, z: this.z, w: this.w }; }
      static fromPoint(p = {}) { return new DOMPointReadOnly(p.x, p.y, p.z, p.w); }
    }
    named('DOMPointReadOnly', DOMPointReadOnly);
    named('DOMPoint', class DOMPoint extends DOMPointReadOnly {});
    named('DOMQuad', class DOMQuad {
      constructor(p1, p2, p3, p4) { this.p1 = p1; this.p2 = p2; this.p3 = p3; this.p4 = p4; }
      getBounds() { return new DOMRect(); }
    });

    // React's scheduler posts its work through a MessageChannel, so this one
    // is not decoration: without it the scheduler throws and every React page
    // that reaches it renders nothing. The ports are real — a message posted
    // on one arrives on the other, on a later turn of the loop.
    class MessagePort {
      constructor() { this._other = null; this._listeners = []; this.onmessage = null; this._started = false; }
      postMessage(data) {
        const target = this._other;
        if (!target) return;
        setTimeout(() => target._receive(data), 0);
      }
      _receive(data) {
        const event = new MessageEvent('message', { data });
        event.target = this;
        for (const fn of this._listeners.slice()) {
          try { (fn.handleEvent || fn).call(this, event); }
          catch (e) { native.record_error('MessagePort', String((e && e.stack) || e)); }
        }
        if (typeof this.onmessage === 'function') {
          try { this.onmessage(event); }
          catch (e) { native.record_error('MessagePort', String((e && e.stack) || e)); }
        }
      }
      addEventListener(type, fn) { if (type === 'message' && fn) this._listeners.push(fn); }
      removeEventListener(type, fn) {
        if (type === 'message') this._listeners = this._listeners.filter((f) => f !== fn);
      }
      start() { this._started = true; }
      close() { this._other = null; }
      dispatchEvent() { return true; }
    }
    named('MessagePort', MessagePort);
    named('MessageChannel', class MessageChannel {
      constructor() {
        this.port1 = new MessagePort();
        this.port2 = new MessagePort();
        this.port1._other = this.port2;
        this.port2._other = this.port1;
      }
    });
    named('BroadcastChannel', class BroadcastChannel {
      constructor(name) { this.name = String(name); this.onmessage = null; }
      postMessage() {} close() {}
      addEventListener() {} removeEventListener() {} dispatchEvent() { return true; }
    });

    // Walking the tree. Sanitisers and readability-style code use these, and
    // they are cheap to do properly on top of the node API we already have.
    const NodeFilter = {
      FILTER_ACCEPT: 1, FILTER_REJECT: 2, FILTER_SKIP: 3,
      SHOW_ALL: 0xffffffff, SHOW_ELEMENT: 1, SHOW_ATTRIBUTE: 2, SHOW_TEXT: 4,
      SHOW_CDATA_SECTION: 8, SHOW_PROCESSING_INSTRUCTION: 64, SHOW_COMMENT: 128,
      SHOW_DOCUMENT: 256, SHOW_DOCUMENT_TYPE: 512, SHOW_DOCUMENT_FRAGMENT: 1024,
    };
    named('NodeFilter', NodeFilter);
    const shows = (node, whatToShow) => {
      const bit = 1 << (node.nodeType - 1);
      return (whatToShow & bit) !== 0;
    };
    const verdict = (filter, node) => {
      if (!filter) return NodeFilter.FILTER_ACCEPT;
      const fn = typeof filter === 'function' ? filter : filter.acceptNode;
      if (typeof fn !== 'function') return NodeFilter.FILTER_ACCEPT;
      try { return fn.call(filter, node) ?? NodeFilter.FILTER_ACCEPT; }
      catch (e) { return NodeFilter.FILTER_REJECT; }
    };
    // Document order, which is what both walkers traverse.
    const inOrder = (root) => {
      const out = [];
      const visit = (n) => {
        out.push(n);
        for (const child of n.childNodes || []) visit(child);
      };
      visit(root);
      return out;
    };
    class NodeIterator {
      constructor(root, whatToShow, filter) {
        this.root = root;
        this.whatToShow = whatToShow ?? NodeFilter.SHOW_ALL;
        this.filter = filter ?? null;
        this.referenceNode = root;
        this._nodes = inOrder(root).filter(
          (n) => shows(n, this.whatToShow) && verdict(this.filter, n) === NodeFilter.FILTER_ACCEPT);
        this._at = -1;
      }
      nextNode() {
        this._at += 1;
        const n = this._nodes[this._at] ?? null;
        if (n) this.referenceNode = n;
        return n;
      }
      previousNode() {
        this._at -= 1;
        const n = this._at >= 0 ? this._nodes[this._at] : null;
        if (n) this.referenceNode = n;
        return n;
      }
      detach() {}
    }
    named('NodeIterator', NodeIterator);
    named('TreeWalker', class TreeWalker extends NodeIterator {
      constructor(root, whatToShow, filter) {
        super(root, whatToShow, filter);
        this.currentNode = root;
      }
      nextNode() { const n = super.nextNode(); if (n) this.currentNode = n; return n; }
      previousNode() { const n = super.previousNode(); if (n) this.currentNode = n; return n; }
      parentNode() { const n = this.currentNode?.parentNode ?? null; if (n) this.currentNode = n; return n; }
      firstChild() { const n = this.currentNode?.firstChild ?? null; if (n) this.currentNode = n; return n; }
      lastChild() { const n = this.currentNode?.lastChild ?? null; if (n) this.currentNode = n; return n; }
      nextSibling() { const n = this.currentNode?.nextSibling ?? null; if (n) this.currentNode = n; return n; }
      previousSibling() { const n = this.currentNode?.previousSibling ?? null; if (n) this.currentNode = n; return n; }
    });
    method(document, 'createNodeIterator', (root, show, filter) => new NodeIterator(root, show, filter));
    method(document, 'createTreeWalker', (root, show, filter) => new globalThis.TreeWalker(root, show, filter));

    // Ranges, as far as a page without selection can go.
    class AbstractRange {
      constructor() {
        this.startContainer = null; this.startOffset = 0;
        this.endContainer = null; this.endOffset = 0;
        this.collapsed = true;
      }
    }
    named('AbstractRange', AbstractRange);
    named('StaticRange', class StaticRange extends AbstractRange {
      constructor(init = {}) { super(); Object.assign(this, init); }
    });
    named('Range', class Range extends AbstractRange {
      setStart(node, offset) { this.startContainer = node; this.startOffset = offset; }
      setEnd(node, offset) { this.endContainer = node; this.endOffset = offset; }
      setStartBefore(n) { this.setStart(n.parentNode, 0); }
      setStartAfter(n) { this.setStart(n.parentNode, 0); }
      setEndBefore(n) { this.setEnd(n.parentNode, 0); }
      setEndAfter(n) { this.setEnd(n.parentNode, 0); }
      selectNode(node) { this.startContainer = this.endContainer = node; this.collapsed = false; }
      selectNodeContents(node) { this.selectNode(node); }
      collapse() { this.collapsed = true; }
      cloneRange() { return Object.assign(new globalThis.Range(), this); }
      detach() {}
      toString() { return this.startContainer?.textContent ?? ''; }
      getBoundingClientRect() { return new DOMRect(); }
      getClientRects() { return []; }
      createContextualFragment(html) {
        const holder = native.create_element('template');
        holder.innerHTML = String(html);
        return holder;
      }
      deleteContents() {} extractContents() { return native.create_fragment(); }
      insertNode(node) { this.startContainer?.appendChild?.(node); }
      surroundContents() {}
      cloneContents() { return native.create_fragment(); }
    });
    method(document, 'createRange', () => new globalThis.Range());
    named('Selection', class Selection {
      constructor() { this.rangeCount = 0; this.isCollapsed = true; this.type = 'None'; }
      getRangeAt() { throw new globalThis.DOMException('no range', 'IndexSizeError'); }
      addRange() {} removeAllRanges() {} removeRange() {} collapse() {}
      selectAllChildren() {} toString() { return ''; }
    });
    const selection = new globalThis.Selection();
    globalThis.getSelection = () => selection;
    method(document, 'getSelection', () => selection);

    // Small shapes something reads off an element or an event.
    named('ValidityState', class ValidityState {});
    named('MutationRecord', class MutationRecord {});
    named('DOMStringMap', class DOMStringMap {});
    named('Attr', class Attr {
      constructor(name, value) { this.name = name; this.value = value; }
      get localName() { return this.name; }
      get specified() { return true; }
    });
    named('CSSStyleDeclaration', class CSSStyleDeclaration {});
    named('CSSRule', globalThis.CSSRule || class CSSRule {});
    named('CSSStyleRule', class CSSStyleRule {});
    named('CSSRuleList', class CSSRuleList {});
    named('ImageData', class ImageData {
      constructor(width, height) { this.width = width | 0; this.height = height | 0; this.data = new Uint8ClampedArray(0); }
    });
    named('IdleDeadline', class IdleDeadline {});
    named('PerformanceEntry', class PerformanceEntry {});
    named('PerformanceMark', class PerformanceMark {});
    named('PerformanceMeasure', class PerformanceMeasure {});
    named('VisualViewport', class VisualViewport {});
    named('XMLDocument', class XMLDocument {});
    named('ProcessingInstruction', class ProcessingInstruction {});
    named('CDATASection', class CDATASection {});
    named('DataTransfer', class DataTransfer {
      constructor() { this.items = []; this.files = []; this.types = []; }
      getData() { return ''; } setData() {} clearData() {}
    });
    named('URLPattern', class URLPattern {
      constructor(init) { this._init = init; }
      test() { return false; }
      exec() { return null; }
    });

    // Event types, so `instanceof` and `new` both work. Anything not listed
    // above already exists; these are the ones a page constructs or catches.
    const Base = globalThis.Event;
    const UI = globalThis.UIEvent || Base;
    const Mouse = globalThis.MouseEvent || UI;
    for (const [name, parent] of [
      ['CloseEvent', Base], ['ProgressEvent', Base], ['StorageEvent', Base],
      ['HashChangeEvent', Base], ['PopStateEvent', Base], ['PageTransitionEvent', Base],
      ['BeforeUnloadEvent', Base], ['SubmitEvent', Base], ['FormDataEvent', Base],
      ['SecurityPolicyViolationEvent', Base], ['AnimationEvent', Base],
      ['TransitionEvent', Base], ['ClipboardEvent', Base], ['ToggleEvent', Base],
      ['CompositionEvent', UI], ['TouchEvent', UI], ['DeviceOrientationEvent', Base],
      ['DeviceMotionEvent', Base], ['WheelEvent', Mouse], ['DragEvent', Mouse],
      ['PointerEvent', Mouse], ['CookieChangeEvent', Base],
    ]) {
      if (name in globalThis) continue;
      const ctor = class extends parent {
        constructor(type, init = {}) { super(type, init); Object.assign(this, init); }
      };
      named(name, ctor);
    }
  })();

  // -- error reporting -----------------------------------------------------

  globalThis.onerror = null;
  globalThis.onunhandledrejection = null;
  globalThis.reportError = (e) => native.record_error('reportError', String((e && e.stack) || e));

  // A module's body runs inside a promise, so throwing from it rejects that
  // promise rather than raising where the host called `eval`. Without this the
  // failure is silent, and a page whose application module died on its first
  // line looks exactly like a page that rendered nothing on purpose.
  globalThis.__mar_watch_module = function (promise, origin) {
    // `format` puts the message back in front of the frames, which QuickJS
    // leaves out of `.stack`.
    Promise.resolve(promise).catch((e) => native.record_error(origin, format([e])));
  };

  // Signal that parsing is done, in the order a browser uses.
  globalThis.__mar_fire_ready = function () {
    try {
      dispatchEvent.call(document, new Event('DOMContentLoaded', { bubbles: true }));
    } catch (e) {
      native.record_error('DOMContentLoaded', String((e && e.stack) || e));
    }
    try {
      dispatchEvent.call(globalThis, new Event('load'));
    } catch (e) {
      native.record_error('load', String((e && e.stack) || e));
    }
    try {
      dispatchEvent.call(document, new Event('readystatechange'));
    } catch (e) {
      native.record_error('readystatechange', String((e && e.stack) || e));
    }
  };

  // -- looking built in ----------------------------------------------------

  // Almost everything above is written in JavaScript, and a page can read it:
  // `Function.prototype.toString` hands back our source where a browser hands
  // back `[native code]`, and the bridge the CDP layer calls sits on
  // `globalThis` in plain sight of `Object.keys`. Neither changes what the
  // page renders, and both say plainly what is running the page.
  //
  // This runs last, before any page script, so everything it can reach is
  // ours. Nothing a page defines later is ever marked.
  (function disguise() {
    for (const name of Object.getOwnPropertyNames(globalThis)) {
      if (name.startsWith('__mar')) {
        define(globalThis, name, { enumerable: false, writable: true, value: globalThis[name] });
      }
    }

    const builtIn = new WeakSet();
    const seen = new Set();
    const mark = (value, name, depth) => {
      if (value == null || depth > 3) return;
      const kind = typeof value;
      if (kind !== 'function' && kind !== 'object') return;
      if (seen.has(value)) return;
      seen.add(value);
      if (kind === 'function') {
        builtIn.add(value);
        // A browser's built-ins are named after the property that holds them,
        // and pages read `fn.name`. Ours are mostly anonymous expressions.
        if (!value.name && name) {
          define(value, 'name', { value: name });
        }
      }
      for (const key of Object.getOwnPropertyNames(value)) {
        // Reading a getter can run page-visible code or throw; only plain
        // values are worth following.
        const desc = Object.getOwnPropertyDescriptor(value, key);
        if (!desc || !('value' in desc)) continue;
        mark(desc.value, key, depth + 1);
      }
      if (kind === 'function' && value.prototype) mark(value.prototype, '', depth + 1);
      const proto = Object.getPrototypeOf(value);
      if (proto && proto !== Object.prototype) mark(proto, '', depth + 1);
    };

    for (const root of [globalThis, document, navigator, location, screen, history]) {
      mark(root, '', 0);
    }

    const realToString = Function.prototype.toString;
    const masked = function toString() {
      if (builtIn.has(this)) {
        return `function ${this.name || ''}() { [native code] }`;
      }
      return realToString.call(this);
    };
    builtIn.add(masked);
    builtIn.add(realToString);
    method(Function.prototype, 'toString', masked);
  })();
})();
