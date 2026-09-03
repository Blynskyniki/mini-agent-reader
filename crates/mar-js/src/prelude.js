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

  globalThis.Event = Event;
  globalThis.CustomEvent = CustomEvent;
  globalThis.MessageEvent = MessageEvent;
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
  const zeroRect = () => ({
    x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0,
    width: 0, height: 0, toJSON() { return { ...this }; },
  });
  method(NodeProto, 'getBoundingClientRect', zeroRect);
  method(NodeProto, 'getClientRects', () => []);
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
  for (const p of ['offsetWidth', 'offsetHeight', 'clientWidth', 'clientHeight',
                   'scrollWidth', 'scrollHeight', 'scrollTop', 'scrollLeft',
                   'offsetTop', 'offsetLeft']) {
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
  method(NodeProto, 'insertAdjacentElement', function (pos, el) {
    this.insertAdjacentHTML(pos, el.outerHTML);
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
      return n === document.documentElement || n === document;
    },
  });

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
  define(document, 'currentScript', { get: () => null, configurable: true });
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
  globalThis.getSelection = () => ({ toString: () => '', rangeCount: 0 });

  // No cascade: computed style is whatever the inline style says.
  globalThis.getComputedStyle = (el) => {
    const inline = styleProxy(el);
    return new Proxy(inline, {
      get(t, p) {
        if (p === 'getPropertyValue') return (n) => t.getPropertyValue(n);
        return t[p];
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
  globalThis.IntersectionObserver = inertObserver({ root: null, rootMargin: '0px', thresholds: [0] });
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
    // No binary pipeline: blobs and buffers surface the text we have.
    blob() { return Promise.resolve({ text: () => Promise.resolve(this._body), size: this._body.length }); }
    arrayBuffer() { return Promise.resolve(new ArrayBuffer(0)); }
    formData() { return Promise.reject(new Error('formData is not supported')); }
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

  globalThis.fetch = (input, init = {}) => {
    const req = input instanceof Request ? input : new Request(input, init);
    const headers = new Headers(init.headers || req.headers).toObject();
    const bodyText = init.body ?? req.body;
    return new Promise((resolve, reject) => {
      let raw;
      try {
        raw = doRequest(req.method, req.url, headers,
          bodyText == null ? null : String(bodyText));
      } catch (e) {
        reject(new TypeError('Failed to fetch: ' + e));
        return;
      }
      if (raw.error && raw.status === 0) {
        reject(new TypeError('Failed to fetch: ' + raw.error));
        return;
      }
      resolve(new Response(raw.body ?? '', {
        status: raw.status, statusText: raw.statusText,
        headers: raw.headers, url: raw.url ?? req.url,
      }));
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
    open(method, url) {
      this._method = method;
      this._url = url;
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
      let raw;
      try {
        raw = doRequest(this._method || 'GET', this._url, this._headers, body ?? null);
      } catch (e) {
        this._fire('error');
        return;
      }
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
    constructor(parts = []) { this._t = parts.join(''); this.size = this._t.length; this.type = ''; }
    text() { return Promise.resolve(this._t); }
    slice() { return this; }
    arrayBuffer() { return Promise.resolve(new ArrayBuffer(0)); }
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

  globalThis.AbortController = class AbortController {
    constructor() {
      this.signal = { aborted: false, reason: undefined,
        addEventListener() {}, removeEventListener() {}, throwIfAborted() {} };
    }
    abort(reason) { this.signal.aborted = true; this.signal.reason = reason; }
  };
  globalThis.AbortSignal = { abort: () => ({ aborted: true }), timeout: () => ({ aborted: false }) };

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

  // -- error reporting -----------------------------------------------------

  globalThis.onerror = null;
  globalThis.onunhandledrejection = null;
  globalThis.reportError = (e) => native.record_error('reportError', String((e && e.stack) || e));

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
