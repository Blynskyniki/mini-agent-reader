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
  const method = (obj, name, fn) => {
    // A browser's built-ins are named after the property that holds them,
    // and pages read `fn.name`; a function expression assigned this way is
    // anonymous unless told otherwise.
    if (typeof fn === 'function' && !fn.name && typeof name === 'string') define(fn, 'name', { value: name });
    return define(obj, name, { writable: true, value: fn });
  };

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

  // What to record for a caught exception. QuickJS keeps only the frames in
  // `.stack`, so recording that alone loses the message — and the message is
  // the part that says what went wrong.
  const describeError = (e) =>
    e instanceof Error ? format([e]) : String((e && e.stack) || e);

  // -- timers -------------------------------------------------------------

  // Extra arguments to setTimeout are passed on to the callback, which some
  // libraries rely on.
  const wrapArgs = (fn, args) =>
    args.length ? () => fn(...args) : fn;
  const asFn = (fn) =>
    typeof fn === 'function' ? fn : () => globalThis.eval(String(fn));

  // A delay is whatever the page passed: `"100"`, `null`, `undefined`, and
  // a browser reads them all as a number of milliseconds or zero.
  const delayOf = (delay) => {
    const n = Number(delay);
    return Number.isFinite(n) && n > 0 ? n : 0;
  };
  globalThis.setTimeout = (fn, delay, ...args) =>
    native.set_timeout(wrapArgs(asFn(fn), args), delayOf(delay));
  globalThis.setInterval = (fn, delay, ...args) =>
    native.set_interval(wrapArgs(asFn(fn), args), delayOf(delay));
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
    // `document.createEvent('Event')` followed by `initEvent` is how a decade
    // of code still fires its own events.
    initEvent(type, bubbles = false, cancelable = false) {
      this.type = String(type);
      this.bubbles = !!bubbles;
      this.cancelable = !!cancelable;
      return this;
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
    initCustomEvent(type, bubbles, cancelable, detail = null) {
      this.initEvent(type, bubbles, cancelable);
      this.detail = detail;
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
    initUIEvent(type, bubbles, cancelable, _view, detail = 0) {
      this.initEvent(type, bubbles, cancelable);
      this.detail = detail;
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
    initMouseEvent(type, bubbles, cancelable, _view, detail, screenX, screenY, clientX, clientY,
      ctrlKey, altKey, shiftKey, metaKey, button, relatedTarget) {
      this.initEvent(type, bubbles, cancelable);
      Object.assign(this, {
        detail: detail ?? 0, screenX: screenX ?? 0, screenY: screenY ?? 0,
        clientX: clientX ?? 0, clientY: clientY ?? 0, pageX: clientX ?? 0, pageY: clientY ?? 0,
        ctrlKey: !!ctrlKey, altKey: !!altKey, shiftKey: !!shiftKey, metaKey: !!metaKey,
        button: button ?? 0, relatedTarget: relatedTarget ?? null,
      });
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
        native.record_error('listener:' + event.type, describeError(e));
      }
    }
    if (typeof inline === 'function' && !event._stoppedImmediate) {
      try {
        inline.call(target, event);
      } catch (e) {
        native.record_error('on' + event.type, describeError(e));
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

  // A node is an EventTarget, and a polyfill that patches
  // `EventTarget.prototype.addEventListener` expects the patch to reach every
  // node through the prototype chain.
  Object.setPrototypeOf(NodeProto, globalThis.EventTarget.prototype);
  define(NodeProto, 'constructor', { value: Node, writable: true });

  // -- element extras ------------------------------------------------------

  // classList over the class attribute. Reads are live; writes go straight back.
  class DOMTokenList {
    constructor(el) {
      Object.defineProperty(this, '_el', { value: el });
    }
    // Read the attribute, not `className`: on an SVG element `className` is
    // an SVGAnimatedString, and the token list is over the attribute anyway.
    get _tokens() {
      return (this._el.getAttribute('class') || '').split(/\s+/).filter(Boolean);
    }
    _write(tokens) {
      this._el.setAttribute('class', tokens.join(' '));
    }
    get length() {
      return this._tokens.length;
    }
    get value() {
      return this._el.getAttribute('class') || '';
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

  globalThis.DOMTokenList = DOMTokenList;
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

  // The methods live on a prototype, because rrweb and its relatives wrap
  // `CSSStyleDeclaration.prototype.setProperty` to watch style changes, and
  // an instance-only method leaves them wrapping `undefined`.
  const styleRead = (el) => parseStyle(el.getAttribute('style'));
  const styleWrite = (el, map) =>
    el.setAttribute('style', [...map].map(([k, v]) => `${k}: ${v}`).join('; '));
  class CSSStyleDeclaration {
    getPropertyValue(p) { return styleRead(this._el).get(dashed(String(p)).toLowerCase()) || ''; }
    getPropertyPriority() { return ''; }
    setProperty(p, v, _priority) {
      const map = styleRead(this._el);
      const key = dashed(String(p)).toLowerCase();
      if (v == null || v === '') map.delete(key); else map.set(key, String(v));
      styleWrite(this._el, map);
    }
    removeProperty(p) {
      const map = styleRead(this._el);
      const key = dashed(String(p)).toLowerCase();
      const old = map.get(key) || '';
      map.delete(key);
      styleWrite(this._el, map);
      return old;
    }
    item(i) { return [...styleRead(this._el).keys()][i] ?? ''; }
    get length() { return styleRead(this._el).size; }
    get cssText() { return this._el.getAttribute('style') || ''; }
    set cssText(v) { this._el.setAttribute('style', String(v)); }
    get parentRule() { return null; }
    [Symbol.iterator]() { return styleRead(this._el).keys(); }
  }
  globalThis.CSSStyleDeclaration = CSSStyleDeclaration;
  function styleProxy(el) {
    const read = () => styleRead(el);
    const api = new CSSStyleDeclaration();
    define(api, '_el', { value: el });
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
    ['type', 'type'],
    ['placeholder', 'placeholder'],
    ['rel', 'rel'],
    ['target', 'target'],
    ['nonce', 'nonce'],
    ['slot', 'slot'],
    ['role', 'role'],
    ['accessKey', 'accesskey'],
    ['htmlFor', 'for'],
    ['autocomplete', 'autocomplete'],
    ['enctype', 'enctype'],
    ['method', 'method'],
    ['pattern', 'pattern'],
    ['step', 'step'],
    ['min', 'min'],
    ['max', 'max'],
    ['label', 'label'],
    ['charset', 'charset'],
    ['crossOrigin', 'crossorigin'],
    ['integrity', 'integrity'],
    ['referrerPolicy', 'referrerpolicy'],
    ['loading', 'loading'],
    ['sizes', 'sizes'],
    ['srcset', 'srcset'],
    ['media', 'media'],
    ['as', 'as'],
    ['hreflang', 'hreflang'],
    ['download', 'download'],
    ['coords', 'coords'],
    ['shape', 'shape'],
    ['useMap', 'usemap'],
    ['wrap', 'wrap'],
    ['headers', 'headers'],
    ['abbr', 'abbr'],
    ['scope', 'scope'],
    ['axis', 'axis'],
    ['cite', 'cite'],
    ['dateTime', 'datetime'],
    ['httpEquiv', 'http-equiv'],
    ['scheme', 'scheme'],
    ['poster', 'poster'],
    ['preload', 'preload'],
    ['kind', 'kind'],
    ['srclang', 'srclang'],
    ['inputMode', 'inputmode'],
    ['enterKeyHint', 'enterkeyhint'],
    ['popover', 'popover'],
    ['popoverTarget', 'popovertarget'],
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

  define(NodeProto, 'name', {
    get() { return this.getAttribute('name') ?? ''; },
    set(v) {
      this.setAttribute('name', String(v));
      if (this.tagName === 'IFRAME' && globalThis.__mar_name_the_document) globalThis.__mar_name_the_document();
    },
  });

  // ARIA reflects the same way: `el.ariaLabel` is `aria-label`.
  for (const prop of [
    'ariaLabel', 'ariaHidden', 'ariaExpanded', 'ariaSelected', 'ariaChecked', 'ariaDisabled',
    'ariaCurrent', 'ariaLive', 'ariaPressed', 'ariaHasPopup', 'ariaModal', 'ariaBusy',
    'ariaAtomic', 'ariaRelevant', 'ariaLevel', 'ariaValueNow', 'ariaValueMin', 'ariaValueMax',
    'ariaValueText', 'ariaOrientation', 'ariaSort', 'ariaMultiSelectable', 'ariaRequired',
    'ariaInvalid', 'ariaReadOnly', 'ariaPlaceholder', 'ariaRoleDescription', 'ariaDescription',
    'ariaKeyShortcuts', 'ariaAutoComplete', 'ariaColCount', 'ariaColIndex', 'ariaRowCount',
    'ariaRowIndex', 'ariaPosInSet', 'ariaSetSize',
  ]) {
    const attr = 'aria-' + prop.slice(4).toLowerCase();
    define(NodeProto, prop, {
      get() { return this.getAttribute(attr); },
      set(v) { if (v == null) this.removeAttribute(attr); else this.setAttribute(attr, String(v)); },
    });
  }

  // `script.text`, `option.text` and `a.text` are the element's text, which
  // is how WordPress reads the JSON it embedded for its own module.
  define(NodeProto, 'text', {
    get() { return this.textContent; },
    set(v) { this.textContent = String(v); },
  });

  // A <template>'s content is a fragment of its own, and it is that fragment
  // a page clones. Every other element reflects the attribute of that name,
  // which is what <meta content> is.
  define(NodeProto, 'content', {
    get() {
      if (this.tagName === 'TEMPLATE') return this.templateContent;
      return this.getAttribute('content') ?? '';
    },
    set(v) {
      if (this.tagName !== 'TEMPLATE') this.setAttribute('content', String(v));
    },
  });

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
    'onkeydown', 'onkeyup', 'onkeypress',
    'onmousedown', 'onmouseup', 'onmouseover', 'onmouseout', 'onmousemove',
    'oncontextmenu', 'onscroll', 'ontouchstart', 'ontouchend', 'ontouchmove', 'ontouchcancel',
    'onmouseenter', 'onmouseleave', 'onwheel', 'onauxclick',
    'onpointerdown', 'onpointerup', 'onpointermove', 'onpointerover', 'onpointerout',
    'onpointerenter', 'onpointerleave', 'onpointercancel',
    'onanimationstart', 'onanimationend', 'onanimationiteration', 'ontransitionend',
    'ontransitionstart', 'ontransitionrun', 'ontransitioncancel',
    'ondragstart', 'ondrag', 'ondragend', 'ondragenter', 'ondragover', 'ondragleave', 'ondrop',
    'onabort', 'oncanplay', 'oncanplaythrough', 'onplay', 'onplaying', 'onpause', 'onended',
    'ontimeupdate', 'onloadeddata', 'onloadedmetadata', 'onloadstart', 'onprogress',
    'onvolumechange', 'onwaiting', 'onseeking', 'onseeked', 'onstalled', 'onsuspend',
    'ontoggle', 'onclose', 'oncancel', 'onselect', 'oninvalid', 'onbeforeinput',
    'onfocusin', 'onfocusout', 'onresize', 'oncopy', 'oncut', 'onpaste', 'onsearch',
    'oncompositionstart', 'oncompositionend', 'oncompositionupdate', 'onselectstart',
    'onselectionchange', 'onvisibilitychange', 'onreadystatechange', 'onslotchange',
    'onformdata', 'onbeforetoggle', 'onscrollend', 'onsecuritypolicyviolation',
    'onload', 'onerror', 'onfocus', 'onblur',
  ]) {
    define(NodeProto, prop, {
      get() {
        const key = keyOf(this) + ':' + prop;
        if (compiled.has(key)) return compiled.get(key);
        // Only an element has an attribute to compile; the document and
        // the window carry the same handler properties without one.
        const source = this.nodeType === 1 ? this.getAttribute(prop) : null;
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
      if (this.tagName === 'SELECT') {
        const chosen = this.querySelector('option[selected]') ?? this.querySelector('option');
        return chosen ? chosen.value : '';
      }
      if (this.tagName === 'OPTION') return this.getAttribute('value') ?? this.textContent.trim();
      const raw = this.getAttribute('value');
      // A checkbox or radio with no value attribute submits "on".
      if (raw == null && this.tagName === 'INPUT' && /^(checkbox|radio)$/i.test(this.getAttribute('type') ?? '')) return 'on';
      return raw ?? '';
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
  method(NodeProto, 'scrollIntoViewIfNeeded', () => {});
  method(NodeProto, 'scrollTo', () => {});
  method(NodeProto, 'scroll', () => {});
  method(NodeProto, 'scrollBy', () => {});

  // The lists the DOM hands back are arrays here, which covers indexing,
  // `length`, `forEach` and iteration; `item()` and `namedItem()` are what a
  // NodeList and an HTMLCollection have on top, and a page calls them.
  const collection = (list) => {
    define(list, 'item', { value: (i) => list[i] ?? null, enumerable: false, writable: true });
    define(list, 'namedItem', {
      value: (n) => list.find((e) => e && e.nodeType === 1 && (e.id === n || e.getAttribute('name') === n)) ?? null,
      enumerable: false,
      writable: true,
    });
    return list;
  };
  for (const name of ['querySelectorAll', 'getElementsByTagName', 'getElementsByClassName']) {
    const original = NodeProto[name];
    method(NodeProto, name, function (...args) { return collection(original.apply(this, args)); });
  }
  for (const name of ['childNodes', 'children']) {
    const original = Object.getOwnPropertyDescriptor(NodeProto, name);
    define(NodeProto, name, { get() { return collection(original.get.call(this)); } });
  }
  method(NodeProto, 'focus', function () {
    activeElement = this;
  });
  method(NodeProto, 'blur', function () {
    if (activeElement === this) activeElement = null;
  });
  method(NodeProto, 'click', function () {
    dispatchEvent.call(this, new Event('click', { bubbles: true, cancelable: true }));
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

  // An attribute as a node of its own. jQuery 1.x reads
  // `el.attributes[name].expando` while it boots, React 19 removes attributes
  // by node during hydration, and both want a NamedNodeMap that answers by
  // index and by name. A plain array answers by index only, and the first
  // named read is a TypeError that takes the library down with it.
  class Attr {
    constructor(owner, name, value) {
      define(this, 'ownerElement', { value: owner ?? null, writable: true });
      define(this, '_name', { value: String(name), writable: true });
      define(this, '_value', { value: value == null ? '' : String(value), writable: true });
    }
    get name() { return this._name; }
    get localName() { return this._name; }
    get nodeName() { return this._name; }
    get prefix() { return null; }
    get namespaceURI() { return null; }
    get specified() { return true; }
    get nodeType() { return 2; }
    get value() {
      const owner = this.ownerElement;
      return owner ? owner.getAttribute(this._name) ?? this._value : this._value;
    }
    set value(v) {
      this._value = String(v);
      if (this.ownerElement) this.ownerElement.setAttribute(this._name, this._value);
    }
    get nodeValue() { return this.value; }
    set nodeValue(v) { this.value = v; }
    get textContent() { return this.value; }
    set textContent(v) { this.value = v; }
    cloneNode() { return new Attr(null, this._name, this.value); }
    toString() { return '[object Attr]'; }
  }
  globalThis.Attr = Attr;

  class NamedNodeMap {
    constructor(el) { define(this, '_el', { value: el }); }
    get _names() { return this._el.getAttributeNames(); }
    get length() { return this._names.length; }
    item(i) {
      const n = this._names[i];
      return n === undefined ? null : new Attr(this._el, n, this._el.getAttribute(n));
    }
    getNamedItem(n) {
      n = String(n).toLowerCase();
      return this._el.hasAttribute(n) ? new Attr(this._el, n, this._el.getAttribute(n)) : null;
    }
    getNamedItemNS(_ns, n) { return this.getNamedItem(n); }
    setNamedItem(attr) {
      const old = this.getNamedItem(attr.name);
      this._el.setAttribute(attr.name, attr.value);
      attr.ownerElement = this._el;
      return old;
    }
    setNamedItemNS(attr) { return this.setNamedItem(attr); }
    removeNamedItem(n) {
      const old = this.getNamedItem(n);
      if (!old) throw new globalThis.DOMException(`No attribute named ${n}`, 'NotFoundError');
      this._el.removeAttribute(n);
      return old;
    }
    removeNamedItemNS(_ns, n) { return this.removeNamedItem(n); }
    [Symbol.iterator]() {
      const el = this._el;
      return this._names.map((n) => new Attr(el, n, el.getAttribute(n)))[Symbol.iterator]();
    }
    forEach(fn, thisArg) { let i = 0; for (const a of this) fn.call(thisArg, a, i++, this); }
  }
  globalThis.NamedNodeMap = NamedNodeMap;
  const isIndex = (p) => typeof p === 'string' && /^\d+$/.test(p);
  const namedNodeMapHandler = {
    get(map, prop, receiver) {
      if (typeof prop === 'symbol' || prop in map) return Reflect.get(map, prop, receiver);
      if (isIndex(prop)) return map.item(Number(prop)) ?? undefined;
      return map.getNamedItem(prop) ?? undefined;
    },
    has(map, prop) {
      if (typeof prop === 'symbol' || prop in map) return true;
      if (isIndex(prop)) return Number(prop) < map.length;
      return map._el.hasAttribute(String(prop).toLowerCase());
    },
    ownKeys(map) { return [...map._names.map((_, i) => String(i)), 'length']; },
    getOwnPropertyDescriptor(map, prop) {
      if (isIndex(prop) && Number(prop) < map.length) {
        return { value: map.item(Number(prop)), enumerable: true, configurable: true };
      }
      if (prop === 'length') return { value: map.length, enumerable: false, configurable: true };
      return Reflect.getOwnPropertyDescriptor(map, prop);
    },
  };
  define(NodeProto, 'attributes', {
    get() { return new Proxy(new NamedNodeMap(this), namedNodeMapHandler); },
  });
  method(NodeProto, 'getAttributeNode', function (name) { return this.attributes.getNamedItem(name); });
  method(NodeProto, 'getAttributeNodeNS', function (_ns, name) { return this.attributes.getNamedItem(name); });
  method(NodeProto, 'setAttributeNode', function (attr) { return this.attributes.setNamedItem(attr); });
  method(NodeProto, 'setAttributeNodeNS', function (attr) { return this.attributes.setNamedItem(attr); });
  method(NodeProto, 'removeAttributeNode', function (attr) {
    const name = attr && attr.name;
    if (name == null || !this.hasAttribute(name)) {
      throw new globalThis.DOMException('The node is not an attribute of this element', 'NotFoundError');
    }
    this.removeAttribute(name);
    attr.ownerElement = null;
    return attr;
  });
  method(NodeProto, 'hasAttributes', function () { return this.getAttributeNames().length > 0; });
  method(NodeProto, 'getAttributeNS', function (_ns, name) { return this.getAttribute(name); });
  method(NodeProto, 'setAttributeNS', function (_ns, name, value) { this.setAttribute(name, value); });
  method(NodeProto, 'hasAttributeNS', function (_ns, name) { return this.hasAttribute(name); });
  method(NodeProto, 'removeAttributeNS', function (_ns, name) { this.removeAttribute(name); });
  method(NodeProto, 'toggleAttribute', NodeProto.toggleAttribute);
  // A component hears about its observed attributes changing, the way it
  // does in a browser. The three writers below are the native ones; the
  // wrappers only add the callback, and only once a component exists.
  const rawSetAttribute = NodeProto.setAttribute;
  const rawRemoveAttribute = NodeProto.removeAttribute;
  const rawToggleAttribute = NodeProto.toggleAttribute;
  const attributeChanged = (el, name, before) => {
    const definition = upgraded.get(keyOf(el));
    if (!definition) return;
    name = String(name).toLowerCase();
    if (!definition.observed.includes(name) || typeof el.attributeChangedCallback !== 'function') return;
    const after = el.getAttribute(name);
    if (before === after) return;
    try { el.attributeChangedCallback(name, before, after, null); }
    catch (e) { native.record_error('attributeChangedCallback:' + definition.name, describeError(e)); }
  };
  method(NodeProto, 'setAttribute', function (name, value) {
    if (upgraded.size === 0) return rawSetAttribute.call(this, name, value);
    const before = this.getAttribute(name);
    rawSetAttribute.call(this, name, value);
    attributeChanged(this, name, before);
  });
  method(NodeProto, 'removeAttribute', function (name) {
    if (upgraded.size === 0) return rawRemoveAttribute.call(this, name);
    const before = this.getAttribute(name);
    rawRemoveAttribute.call(this, name);
    attributeChanged(this, name, before);
  });
  method(NodeProto, 'toggleAttribute', function (name, force) {
    if (upgraded.size === 0) return rawToggleAttribute.call(this, name, force);
    const before = this.getAttribute(name);
    const result = rawToggleAttribute.call(this, name, force);
    attributeChanged(this, name, before);
    return result;
  });

  // Identity and order. `isEqualNode` is how react-helmet decides whether a
  // tag it is about to insert is already there; `compareDocumentPosition` is
  // how a stylesheet manager sorts what it inserted.
  method(NodeProto, 'isSameNode', function (other) { return !!other && keyOf(other) === keyOf(this); });
  method(NodeProto, 'isEqualNode', function (other) {
    if (!other || other.nodeType !== this.nodeType) return false;
    if (this.nodeType === 1) {
      if (this.tagName !== other.tagName) return false;
      const mine = this.getAttributeNames();
      if (mine.length !== other.getAttributeNames().length) return false;
      for (const name of mine) if (this.getAttribute(name) !== other.getAttribute(name)) return false;
    } else if (this.nodeType === 3 || this.nodeType === 8) {
      return this.data === other.data;
    }
    const a = this.childNodes, b = other.childNodes;
    if (a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (!a[i].isEqualNode(b[i])) return false;
    return true;
  });
  // The arena root stands in for `document`, and the bindings hide it as a
  // parent; walking up therefore stops at <html>, which is one short.
  const parentOf = (n) => {
    const p = n.parentNode;
    if (p) return p;
    const root = native.document_element();
    return root && n.nodeType === 1 && keyOf(n) === keyOf(root) ? native.root() : null;
  };
  method(NodeProto, 'getRootNode', function () {
    let n = this;
    for (let up = parentOf(n); up; up = parentOf(n)) n = up;
    return n;
  });
  method(NodeProto, 'compareDocumentPosition', function (other) {
    if (!other || typeof other.nodeType !== 'number') throw new TypeError('compareDocumentPosition: not a node');
    if (keyOf(other) === keyOf(this)) return 0;
    const chain = (n) => { const out = [n]; for (let up = parentOf(n); up; up = parentOf(up)) out.push(up); return out; };
    const a = chain(this), b = chain(other);
    const ka = a.map(keyOf), kb = b.map(keyOf);
    if (ka[ka.length - 1] !== kb[kb.length - 1]) return 1 | 32 | (ka[0] < kb[0] ? 4 : 2);
    if (ka.includes(kb[0])) return 2 | 8;
    if (kb.includes(ka[0])) return 4 | 16;
    let i = ka.length - 1, j = kb.length - 1;
    while (i > 0 && j > 0 && ka[i - 1] === kb[j - 1]) { i--; j--; }
    const siblings = a[i].childNodes.map(keyOf);
    return siblings.indexOf(ka[i - 1]) < siblings.indexOf(kb[j - 1]) ? 4 : 2;
  });
  method(NodeProto, 'lookupNamespaceURI', (prefix) => (prefix == null || prefix === '' ? 'http://www.w3.org/1999/xhtml' : null));
  method(NodeProto, 'lookupPrefix', () => null);
  method(NodeProto, 'isDefaultNamespace', (ns) => ns === 'http://www.w3.org/1999/xhtml');

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
  // Each interface gets a prototype of its own, chained the way the DOM
  // chains them, and the bridge hands a node out with the prototype for its
  // tag. A page that patches `HTMLTemplateElement.prototype` then reaches
  // templates and nothing else, and `div.constructor.name` says what it is.
  const domInterface = (name, holds, parent = NodeProto) => {
    const ctor = function () {
      throw new TypeError(`Illegal constructor: ${name}`);
    };
    define(ctor, 'name', { value: name });
    define(ctor, Symbol.hasInstance, { value: holds });
    ctor.prototype = Object.create(parent, {
      constructor: { value: ctor, writable: true, configurable: true },
    });
    define(ctor.prototype, Symbol.toStringTag, { value: name });
    globalThis[name] = ctor;
    return ctor;
  };
  const isType = (type) => (v) => !!v && v.nodeType === type;
  // `EventTarget` is not in this list: it is defined above as a real class,
  // because a page that writes `class Bus extends EventTarget` has to be able
  // to construct one.
  const Element = domInterface('Element', isType(1));
  const HTMLElement = domInterface('HTMLElement', isType(1), Element.prototype);
  // `class X extends HTMLElement` reaches this through `super()`. During an
  // upgrade the element already exists and is handed back; `new X()` from
  // page code creates one named for the definition.
  const htmlElementCtor = function HTMLElement() {
    const pending = globalThis.__mar_element_under_construction?.();
    if (pending) {
      // The class's methods must be reachable from `this` before the
      // class's own constructor body runs: Lit calls one on its first line.
      if (new.target && new.target.prototype) Object.setPrototypeOf(pending, new.target.prototype);
      return pending;
    }
    const ctor = new.target;
    const name = ctor && globalThis.customElements?.getName?.(ctor);
    if (name) {
      const el = native.create_element(name);
      Object.setPrototypeOf(el, ctor.prototype);
      return el;
    }
    throw new TypeError('Illegal constructor: HTMLElement');
  };
  htmlElementCtor.prototype = HTMLElement.prototype;
  HTMLElement.prototype.constructor = htmlElementCtor;
  define(htmlElementCtor, 'name', { value: 'HTMLElement' });
  // Any element is an HTMLElement; a subclass — a custom element's class —
  // answers for its own prototype chain and nothing more.
  define(htmlElementCtor, Symbol.hasInstance, {
    value: function (v) {
      if (this === htmlElementCtor) return isType(1)(v);
      return Function.prototype[Symbol.hasInstance].call(this, v);
    },
  });
  globalThis.HTMLElement = htmlElementCtor;
  // `new Text('x')` and `new DocumentFragment()` are ordinary code, so these
  // three construct; the rest of the DOM's interfaces throw as they should.
  const constructible = (name, make, holds, parent) => {
    const ctor = function (...args) {
      const node = make(...args);
      Object.setPrototypeOf(node, ctor.prototype);
      return node;
    };
    define(ctor, 'name', { value: name });
    define(ctor, Symbol.hasInstance, { value: holds });
    ctor.prototype = Object.create(parent, {
      constructor: { value: ctor, writable: true, configurable: true },
    });
    define(ctor.prototype, Symbol.toStringTag, { value: name });
    globalThis[name] = ctor;
    return ctor;
  };
  const CharacterData = domInterface('CharacterData', (v) => !!v && (v.nodeType === 3 || v.nodeType === 8));
  constructible('Text', (data = '') => native.create_text_node(String(data)), isType(3), CharacterData.prototype);
  constructible('Comment', (data = '') => native.create_comment(String(data)), isType(8), CharacterData.prototype);
  const shadowHosts = new Map();
  const DocumentFragment = constructible('DocumentFragment', () => native.create_fragment(),
    (v) => !!v && v.nodeType === 11 && !shadowHosts.has(keyOf(v)), NodeProto);
  const Document = domInterface('Document', isType(9));
  domInterface('HTMLDocument', isType(9), Document.prototype);
  domInterface('XMLDocument', () => false, Document.prototype);
  domInterface('DocumentType', isType(10));
  // A shadow root is a fragment that knows its host. It gets a prototype of
  // its own so that `host` and `innerHTML` are there and not on every
  // fragment: ShadyDOM builds its roots on DocumentFragment.prototype and
  // assigns `this.host` expecting a plain property.
  const ShadowRoot = domInterface('ShadowRoot', (v) => !!v && v.nodeType === 11 && shadowHosts.has(keyOf(v)), DocumentFragment.prototype);
  // Nothing here parses SVG into SVG-aware nodes, but an element with an SVG
  // tag name is still what a script means by one.
  const SVG_TAGS = ['svg', 'path', 'circle', 'rect', 'g', 'line', 'polyline', 'polygon', 'ellipse',
    'tspan', 'defs', 'use', 'symbol', 'clippath', 'mask', 'pattern', 'lineargradient',
    'radialgradient', 'stop', 'filter', 'foreignobject', 'marker', 'desc', 'fecolormatrix',
    'fegaussianblur', 'feoffset', 'feblend', 'femerge', 'femergenode', 'feflood', 'fecomposite',
    'animate', 'animatetransform', 'text'];
  const isSvg = (v) => !!v && v.nodeType === 1 && SVG_TAGS.includes(String(v.tagName).toLowerCase());
  const SVGElement = domInterface('SVGElement', isSvg, Element.prototype);
  const SVGGraphicsElement = domInterface('SVGGraphicsElement', isSvg, SVGElement.prototype);
  const SVGSVGElement = domInterface('SVGSVGElement', (v) => isSvg(v) && v.tagName === 'SVG', SVGGraphicsElement.prototype);
  define(SVGElement.prototype, 'className', {
    get() { const v = this.getAttribute('class') ?? ''; return { baseVal: v, animVal: v }; },
    set(v) { this.setAttribute('class', String(v && v.baseVal !== undefined ? v.baseVal : v)); },
  });
  define(SVGElement.prototype, 'ownerSVGElement', { get() { return this.closest('svg'); } });
  define(SVGElement.prototype, 'viewportElement', { get() { return this.closest('svg'); } });
  method(SVGGraphicsElement.prototype, 'getBBox', function () {
    const r = this.getBoundingClientRect();
    return { x: 0, y: 0, width: r.width, height: r.height };
  });
  method(SVGGraphicsElement.prototype, 'getCTM', () => null);
  method(SVGGraphicsElement.prototype, 'getScreenCTM', () => null);
  method(SVGSVGElement.prototype, 'createSVGPoint', () => ({ x: 0, y: 0, matrixTransform() { return this; } }));
  method(SVGSVGElement.prototype, 'createSVGMatrix', () => new globalThis.DOMMatrix());
  method(SVGSVGElement.prototype, 'createSVGRect', () => ({ x: 0, y: 0, width: 0, height: 0 }));
  method(SVGSVGElement.prototype, 'getElementById', function (id) { return this.querySelector(`[id="${String(id).replace(/"/g, '\\"')}"]`); });
  define(SVGSVGElement.prototype, 'viewBox', {
    get() {
      const [x = 0, y = 0, w = 0, h = 0] = (this.getAttribute('viewBox') ?? '').trim().split(/[\s,]+/).map(Number);
      const box = { x, y, width: w, height: h };
      return { baseVal: box, animVal: box };
    },
  });

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
  const anyOf = (...tags) => (v) => !!v && v.nodeType === 1 && tags.includes(v.tagName);
  const HTMLMediaElement = domInterface('HTMLMediaElement', anyOf('AUDIO', 'VIDEO'), HTMLElement.prototype);
  const tagPrototypes = {};
  for (const [name, tag] of Object.entries(TAG_INTERFACES)) {
    const parent = tag === 'AUDIO' || tag === 'VIDEO' ? HTMLMediaElement.prototype : HTMLElement.prototype;
    const ctor = domInterface(name, (v) => !!v && v.nodeType === 1 && v.tagName === tag, parent);
    tagPrototypes[tag.toLowerCase()] = ctor.prototype;
  }
  // Interfaces one tag name cannot answer for.
  for (const [name, tags] of [
    ['HTMLHeadingElement', ['H1', 'H2', 'H3', 'H4', 'H5', 'H6']],
    ['HTMLTableCellElement', ['TD', 'TH']],
    ['HTMLTableSectionElement', ['THEAD', 'TBODY', 'TFOOT']],
    ['HTMLQuoteElement', ['BLOCKQUOTE', 'Q']],
    ['HTMLModElement', ['INS', 'DEL']],
  ]) {
    const ctor = domInterface(name, anyOf(...tags), HTMLElement.prototype);
    for (const tag of tags) tagPrototypes[tag.toLowerCase()] = ctor.prototype;
  }
  domInterface('HTMLUnknownElement', () => false, HTMLElement.prototype);
  for (const tag of SVG_TAGS) tagPrototypes[tag] = tag === 'svg' ? SVGSVGElement.prototype : SVGGraphicsElement.prototype;
  // Hand the bridge the map, and give the document — wrapped before any of
  // this existed — the prototype it would have been created with.
  native.register_prototypes({
    element: HTMLElement.prototype,
    document: globalThis.HTMLDocument.prototype,
    fragment: DocumentFragment.prototype,
    text: globalThis.Text.prototype,
    comment: globalThis.Comment.prototype,
    doctype: globalThis.DocumentType.prototype,
  }, tagPrototypes);

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
    ctor.prototype = tagPrototypes[tag] ?? HTMLElement.prototype;
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
    DOCUMENT_POSITION_DISCONNECTED: 1, DOCUMENT_POSITION_PRECEDING: 2,
    DOCUMENT_POSITION_FOLLOWING: 4, DOCUMENT_POSITION_CONTAINS: 8,
    DOCUMENT_POSITION_CONTAINED_BY: 16, DOCUMENT_POSITION_IMPLEMENTATION_SPECIFIC: 32,
  })) {
    define(Node, name, { value });
    define(NodeProto, name, { value });
  }

  // -- document ------------------------------------------------------------

  let activeElement = null;

  const documentNode = native.root();
  const document = documentNode;
  Object.setPrototypeOf(document, globalThis.HTMLDocument.prototype);
  // Methods go on the prototype, because that is where a consent manager
  // looks for `createElement` to wrap it, and an own method on the instance
  // leaves it wrapping `undefined`.
  const DocumentProto = globalThis.Document.prototype;

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

  method(DocumentProto, 'createElement', (n) => {
    const el = native.create_element(n);
    const definition = globalThis.customElements?.get?.(String(n).toLowerCase());
    if (definition && globalThis.__mar_upgrade_within) globalThis.__mar_upgrade_within(el);
    return el;
  });
  method(DocumentProto, 'createElementNS', (_ns, n) => native.create_element(n));
  method(DocumentProto, 'createTextNode', (t) => native.create_text_node(String(t)));
  method(DocumentProto, 'createComment', (t) => native.create_comment(String(t)));
  method(DocumentProto, 'createDocumentFragment', () => native.create_fragment());
  method(DocumentProto, 'getElementById', (id) => native.get_element_by_id(String(id)));
  method(DocumentProto, 'write', (...s) => {
    // document.write after parsing would replace the page in a real browser.
    // Appending is the safer reading of what the script intended.
    const body = native.body();
    if (body) body.insertAdjacentHTML('beforeend', s.join(''));
  });
  method(DocumentProto, 'writeln', (...s) => document.write(...s, '\n'));
  method(DocumentProto, 'open', () => document);
  method(DocumentProto, 'close', () => {});
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
        if (title !== undefined) {
          const el = native.create_element('title');
          el.textContent = String(title);
          doc.querySelector('head').appendChild(el);
        }
        return globalThis.__mar_document_like(doc);
      },
      createDocument() { return this.createHTMLDocument(); },
      createDocumentType: (name) => ({ name, publicId: '', systemId: '' }),
    }),
  });

  method(DocumentProto, 'importNode', (n, deep) => n.cloneNode(deep));
  method(DocumentProto, 'adoptNode', (n) => n);
  method(DocumentProto, 'createRange', () => ({
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
  method(DocumentProto, 'createTreeWalker', (root) => {
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
  method(DocumentProto, 'elementFromPoint', () => null);
  method(DocumentProto, 'hasFocus', () => true);
  method(DocumentProto, 'execCommand', () => false);
  method(DocumentProto, 'evaluate', () => ({ iterateNext: () => null, snapshotLength: 0 }));
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
  // Read on demand, not once: the host sets the user agent to whatever its
  // handshake claimed, and it does so after this prelude has run. A page
  // that hashes `navigator.userAgent` and a server that checks the hash
  // against the header must see the same string.
  const ua = () => native.user_agent();
  const chromeMajor = () => (/Chrome\/(\d+)/.exec(ua()) || [, '140'])[1];

  globalThis.navigator = {
    get userAgent() { return ua(); },
    get appVersion() { return ua().replace('Mozilla/', ''); },
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
    serviceWorker: {
      controller: null, oncontrollerchange: null, onmessage: null, onmessageerror: null,
      register: () => Promise.reject(new globalThis.DOMException('service workers are not available here', 'SecurityError')),
      getRegistration: () => Promise.resolve(undefined),
      getRegistrations: () => Promise.resolve([]),
      ready: new Promise(() => {}),
      startMessages() {},
      addEventListener() {}, removeEventListener() {}, dispatchEvent() { return true; },
    },
    get userAgentData() {
      const version = chromeMajor();
      return {
        brands: [
          { brand: 'Chromium', version },
          { brand: 'Google Chrome', version },
          { brand: 'Not=A?Brand', version: '24' },
        ],
        mobile: false,
        platform: 'macOS',
        getHighEntropyValues: () => Promise.resolve({
          architecture: 'x86', bitness: '64', model: '', platform: 'macOS',
          platformVersion: '15.0.0', uaFullVersion: `${version}.0.0.0`, fullVersionList: [
            { brand: 'Chromium', version: `${version}.0.0.0` },
            { brand: 'Google Chrome', version: `${version}.0.0.0` },
          ],
        }),
        toJSON() { return { brands: this.brands, mobile: false, platform: 'macOS' }; },
      };
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
          native.record_error('IntersectionObserver', describeError(e));
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
          native.record_error('MutationObserver', describeError(e));
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

  // zone.js patches `addEventListener` on `XMLHttpRequestEventTarget`'s
  // prototype and reads `readyState === DONE` off the instance; without the
  // class and the constants, Angular's HttpClient never sees a response.
  class XMLHttpRequestEventTarget extends globalThis.EventTarget {
    constructor() {
      super();
      this._listeners = {};
    }
    addEventListener(t, fn) { (this._listeners[String(t)] ||= []).push(fn); }
    removeEventListener(t, fn) {
      const l = this._listeners[String(t)];
      if (l) this._listeners[String(t)] = l.filter((f) => f !== fn);
    }
    dispatchEvent(ev) { this._fire(ev.type, ev); return !ev.defaultPrevented; }
    _fire(type, ev) {
      ev = ev || new Event(type);
      ev.target = this;
      for (const fn of (this._listeners[type] || []).slice()) {
        try { (fn.handleEvent || fn).call(this, ev); } catch (e) { native.record_error('xhr:' + type, describeError(e)); }
      }
      const inline = this['on' + type];
      if (typeof inline === 'function') {
        try { inline.call(this, ev); } catch (e) { native.record_error('xhr:on' + type, describeError(e)); }
      }
    }
  }
  globalThis.XMLHttpRequestEventTarget = XMLHttpRequestEventTarget;
  globalThis.XMLHttpRequestUpload = class XMLHttpRequestUpload extends XMLHttpRequestEventTarget {};
  globalThis.XMLHttpRequest = class XMLHttpRequest extends XMLHttpRequestEventTarget {
    constructor() {
      super();
      this.readyState = 0;
      this.status = 0;
      this.statusText = '';
      this.responseText = '';
      this.response = '';
      this.responseType = '';
      this.responseURL = '';
      this.timeout = 0;
      this.withCredentials = false;
      this.upload = new globalThis.XMLHttpRequestUpload();
      this._headers = {};
      this._responseHeaders = {};
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

  for (const [name, value] of [['UNSENT', 0], ['OPENED', 1], ['HEADERS_RECEIVED', 2], ['LOADING', 3], ['DONE', 4]]) {
    define(globalThis.XMLHttpRequest, name, { value, enumerable: true });
    define(globalThis.XMLHttpRequest.prototype, name, { value, enumerable: true });
  }

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
  // A Worker, on this thread. There is one interpreter and one event loop
  // here, so the worker's script runs in the same context as the page,
  // inside a scope that shadows what a worker cannot see — `window`,
  // `document` — and supplies what it has instead: `self`, `postMessage`,
  // `importScripts`. Messages cross in both directions on the next turn of
  // the loop, as they would between threads. A bot check that hashes in a
  // worker and posts the answer back gets its answer; a page that keeps its
  // application in one gets its application.
  const objectUrls = new Map();
  let nextObjectUrl = 1;
  globalThis.__mar_object_url = {
    create(object) {
      const url = `blob:${location.origin}/mar-${nextObjectUrl++}`;
      objectUrls.set(url, object);
      return url;
    },
    revoke(url) { objectUrls.delete(String(url)); },
    get(url) { return objectUrls.get(String(url)); },
  };
  const workerSource = (url) => {
    const text = String(url);
    if (text.startsWith('blob:')) {
      const blob = objectUrls.get(text);
      if (!blob) throw new Error(`blob URL not found: ${text}`);
      return typeof blob._t === 'string' ? blob._t : String(blob);
    }
    if (text.startsWith('data:')) {
      const comma = text.indexOf(',');
      const meta = text.slice(5, comma);
      const payload = text.slice(comma + 1);
      return /;base64$/i.test(meta) ? globalThis.atob(payload) : decodeURIComponent(payload);
    }
    const absolute = new URL(text, location.href).href;
    const raw = doRequest('GET', absolute, { Accept: '*/*' }, null);
    if (!raw || !raw.ok) throw new Error(`worker script ${absolute}: ${raw && (raw.error || raw.status)}`);
    return raw.body ?? '';
  };
  const workerListeners = (holder) => ({
    add(type, fn) { (holder[type] ||= []).push(fn); },
    remove(type, fn) { if (holder[type]) holder[type] = holder[type].filter((f) => f !== fn); },
    fire(target, type, event, inline, onError) {
      event.target = target;
      const failed = onError || ((e) => native.record_error('worker:' + type, describeError(e)));
      for (const fn of (holder[type] || []).slice()) {
        try { (fn.handleEvent || fn).call(target, event); } catch (e) { failed(e); }
      }
      if (typeof inline === 'function') {
        try { inline.call(target, event); } catch (e) { failed(e); }
      }
    },
  });
  // The names a script declares at its top level, so that what one
  // `importScripts` defined the next script can see: a function's
  // declarations are its own, and each script runs as a function here.
  const DECLARED = /\b(?:function\*?\s+|(?:var|let|const|class)\s+)([A-Za-z_$][\w$]*)/g;
  const exportsOf = (source) => {
    const names = new Set();
    for (const m of source.matchAll(DECLARED)) names.add(m[1]);
    return [...names]
      .map((n) => `try { __mar_scope[${JSON.stringify(n)}] = ${n}; } catch (e) {}`)
      .join('\n');
  };
  class WorkerGlobalScope {}
  class DedicatedWorkerGlobalScope extends WorkerGlobalScope {}
  globalThis.WorkerGlobalScope = WorkerGlobalScope;
  globalThis.DedicatedWorkerGlobalScope = DedicatedWorkerGlobalScope;
  globalThis.WorkerNavigator = class WorkerNavigator {};
  globalThis.WorkerLocation = class WorkerLocation {};
  globalThis.Worker = class Worker extends globalThis.EventTarget {
    constructor(url, options = {}) {
      super();
      const worker = this;
      const outside = workerListeners({});
      const inside = workerListeners({});
      let alive = true;
      let ready = false;
      const queued = [];
      const scope = Object.create(DedicatedWorkerGlobalScope.prototype);
      const scriptUrl = (() => { try { return new URL(String(url), location.href).href; } catch { return String(url); } })();
      const errorOut = (e) => {
        const event = new globalThis.ErrorEvent('error', {});
        event.message = e instanceof Error ? e.message : String(e);
        event.error = e;
        event.filename = scriptUrl;
        setTimeout(() => outside.fire(worker, 'error', event, worker.onerror), 0);
      };
      const toMain = (data) => {
        if (!alive) return;
        setTimeout(() => {
          if (!alive) return;
          outside.fire(worker, 'message', new MessageEvent('message', { data }), worker.onmessage);
        }, 0);
      };
      const toWorker = (data) => {
        if (!alive) return;
        if (!ready) { queued.push(data); return; }
        setTimeout(() => {
          if (!alive) return;
          inside.fire(scope, 'message', new MessageEvent('message', { data }), scope.onmessage, (e) => {
            native.record_error('Worker ' + scriptUrl, describeError(e));
            errorOut(e);
          });
        }, 0);
      };
      const evaluate = (source, name) => {
        // `with` makes the scope's names win over the page's, and lets a
        // bare `onmessage = ...` land on the scope rather than on nothing.
        const run = new Function('__mar_scope',
          `with (__mar_scope) {\n${source}\n;${exportsOf(source)}\n}\n//# sourceURL=${name}`);
        run.call(scope, scope);
      };
      Object.assign(scope, {
        self: scope, globalThis: scope, window: undefined, document: undefined,
        frames: undefined, parent: undefined, top: undefined, frameElement: undefined,
        onmessage: null, onerror: null, onmessageerror: null,
        name: String(options.name ?? ''),
        location: Object.assign(Object.create(globalThis.WorkerLocation.prototype), {
          href: scriptUrl, origin: location.origin, protocol: location.protocol,
          host: location.host, hostname: location.hostname, port: location.port,
          pathname: (() => { try { return new URL(scriptUrl).pathname; } catch { return '/'; } })(),
          search: '', hash: '', toString: () => scriptUrl,
        }),
        navigator: Object.assign(Object.create(globalThis.WorkerNavigator.prototype), navigator),
        postMessage: (data) => toMain(data),
        close: () => { alive = false; },
        importScripts: (...urls) => {
          for (const u of urls) {
            const absolute = new URL(String(u), scriptUrl).href;
            evaluate(workerSource(absolute), absolute);
          }
        },
        addEventListener: (type, fn) => inside.add(String(type), fn),
        removeEventListener: (type, fn) => inside.remove(String(type), fn),
        dispatchEvent: (event) => { inside.fire(scope, event.type, event, scope['on' + event.type]); return true; },
        WorkerGlobalScope, DedicatedWorkerGlobalScope,
      });
      define(this, 'onmessage', { value: null, writable: true });
      define(this, 'onerror', { value: null, writable: true });
      define(this, 'onmessageerror', { value: null, writable: true });
      define(this, '_outside', { value: outside });
      define(this, '_send', { value: toWorker });
      define(this, '_stop', { value: () => { alive = false; } });
      // The script runs on the next turn, as a real worker starts after the
      // constructor returns; messages posted before then are kept in order.
      setTimeout(() => {
        if (!alive) return;
        try {
          const source = workerSource(url);
          if (String(options.type).toLowerCase() === 'module') {
            // No scope trick for a module; it sees the page's globals plus
            // a `self` that is the scope, which is what most of them touch.
            const previousSelf = globalThis.self;
            globalThis.self = scope;
            try { runModuleSource(source, scriptUrl); } finally { globalThis.self = previousSelf; }
          } else {
            evaluate(source, scriptUrl);
          }
          ready = true;
          for (const data of queued.splice(0)) toWorker(data);
        } catch (e) {
          native.record_error('Worker ' + scriptUrl, describeError(e));
          errorOut(e);
        }
      }, 0);
    }
    postMessage(data) { this._send(data); }
    terminate() { this._stop(); }
    addEventListener(type, fn) { this._outside.add(String(type), fn); }
    removeEventListener(type, fn) { this._outside.remove(String(type), fn); }
    dispatchEvent(event) { this._outside.fire(this, event.type, event, this['on' + event.type]); return true; }
  };
  globalThis.SharedWorker = class SharedWorker {
    constructor(url, options) {
      const worker = new globalThis.Worker(url, typeof options === 'string' ? { name: options } : options);
      this.port = {
        postMessage: (d) => worker.postMessage(d), start() {}, close() { worker.terminate(); },
        addEventListener: (t, f) => worker.addEventListener(t, f),
        removeEventListener: (t, f) => worker.removeEventListener(t, f),
        set onmessage(fn) { worker.onmessage = fn; }, get onmessage() { return worker.onmessage; },
      };
      this.onerror = null;
    }
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
    static createObjectURL(object) { return globalThis.__mar_object_url.create(object); }
    static revokeObjectURL(url) { globalThis.__mar_object_url.revoke(url); }
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
      // with the same node API as the page itself, and a sanitiser reads
      // `.body` off it, so it answers as a document does.
      const holder = native.create_element('html');
      holder.innerHTML = String(html);
      return globalThis.__mar_document_like(holder);
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
    subtle: {
      // A bot check hashes in a worker and asks `subtle.digest` for it; a
      // library fingerprints itself the same way. SHA-1 and SHA-256 are the
      // ones asked for, and both are short enough to carry here.
      digest(algorithm, data) {
        const name = String(typeof algorithm === 'string' ? algorithm : algorithm && algorithm.name).toUpperCase().replace('-', '');
        const bytes = data instanceof ArrayBuffer ? new Uint8Array(data)
          : ArrayBuffer.isView(data) ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength)
          : new TextEncoder().encode(String(data));
        return new Promise((resolve, reject) => {
          if (name === 'SHA256') resolve(sha256(bytes).buffer);
          else if (name === 'SHA1') resolve(sha1(bytes).buffer);
          else reject(new globalThis.DOMException(`digest ${algorithm} is not supported here`, 'NotSupportedError'));
        });
      },
      importKey: () => Promise.reject(new Error('unsupported')),
      generateKey: () => Promise.reject(new Error('unsupported')),
      encrypt: () => Promise.reject(new Error('unsupported')),
      decrypt: () => Promise.reject(new Error('unsupported')),
      sign: () => Promise.reject(new Error('unsupported')),
      verify: () => Promise.reject(new Error('unsupported')),
    },
  };
  function shaPad(bytes, blockBytes) {
    const bitLength = bytes.length * 8;
    const total = Math.ceil((bytes.length + 9) / blockBytes) * blockBytes;
    const padded = new Uint8Array(total);
    padded.set(bytes);
    padded[bytes.length] = 0x80;
    const view = new DataView(padded.buffer);
    view.setUint32(total - 4, bitLength >>> 0);
    view.setUint32(total - 8, Math.floor(bitLength / 0x100000000));
    return padded;
  }
  function sha256(bytes) {
    const K = [
      0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
      0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
      0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
      0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
      0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
      0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
      0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
      0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    const H = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    const padded = shaPad(bytes, 64);
    const view = new DataView(padded.buffer);
    const w = new Uint32Array(64);
    const rotr = (x, n) => (x >>> n) | (x << (32 - n));
    for (let offset = 0; offset < padded.length; offset += 64) {
      for (let i = 0; i < 16; i++) w[i] = view.getUint32(offset + i * 4);
      for (let i = 16; i < 64; i++) {
        const s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >>> 3);
        const s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >>> 10);
        w[i] = (w[i - 16] + s0 + w[i - 7] + s1) >>> 0;
      }
      let [a, b, c, d, e, f, g, h] = H;
      for (let i = 0; i < 64; i++) {
        const S1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
        const ch = (e & f) ^ (~e & g);
        const t1 = (h + S1 + ch + K[i] + w[i]) >>> 0;
        const S0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
        const maj = (a & b) ^ (a & c) ^ (b & c);
        const t2 = (S0 + maj) >>> 0;
        h = g; g = f; f = e; e = (d + t1) >>> 0; d = c; c = b; b = a; a = (t1 + t2) >>> 0;
      }
      H[0] = (H[0] + a) >>> 0; H[1] = (H[1] + b) >>> 0; H[2] = (H[2] + c) >>> 0; H[3] = (H[3] + d) >>> 0;
      H[4] = (H[4] + e) >>> 0; H[5] = (H[5] + f) >>> 0; H[6] = (H[6] + g) >>> 0; H[7] = (H[7] + h) >>> 0;
    }
    const out = new Uint8Array(32);
    const outView = new DataView(out.buffer);
    H.forEach((v, i) => outView.setUint32(i * 4, v));
    return out;
  }
  function sha1(bytes) {
    const H = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0];
    const padded = shaPad(bytes, 64);
    const view = new DataView(padded.buffer);
    const w = new Uint32Array(80);
    const rotl = (x, n) => (x << n) | (x >>> (32 - n));
    for (let offset = 0; offset < padded.length; offset += 64) {
      for (let i = 0; i < 16; i++) w[i] = view.getUint32(offset + i * 4);
      for (let i = 16; i < 80; i++) w[i] = rotl(w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16], 1);
      let [a, b, c, d, e] = H;
      for (let i = 0; i < 80; i++) {
        const [f, k] = i < 20 ? [(b & c) | (~b & d), 0x5a827999]
          : i < 40 ? [b ^ c ^ d, 0x6ed9eba1]
          : i < 60 ? [(b & c) | (b & d) | (c & d), 0x8f1bbcdc]
          : [b ^ c ^ d, 0xca62c1d6];
        const t = (rotl(a, 5) + f + e + k + w[i]) >>> 0;
        e = d; d = c; c = rotl(b, 30); b = a; a = t;
      }
      H[0] = (H[0] + a) >>> 0; H[1] = (H[1] + b) >>> 0; H[2] = (H[2] + c) >>> 0; H[3] = (H[3] + d) >>> 0; H[4] = (H[4] + e) >>> 0;
    }
    const out = new Uint8Array(20);
    const outView = new DataView(out.buffer);
    H.forEach((v, i) => outView.setUint32(i * 4, v));
    return out;
  }

  // A page asks `CSS.supports` to decide whether the browser is too old to
  // serve, and sends it to a "please update" page on a no. Nothing here lays
  // out, so no answer is checked against anything; yes is the answer that
  // gets the page.
  globalThis.CSS = {
    supports: () => true,
    escape: (s) => String(s).replace(/[^\w-]/g, '\\$&'),
    registerProperty() {},
    px: (v) => ({ value: v, unit: 'px' }),
    number: (v) => ({ value: v, unit: 'number' }),
    highlights: new Map(),
  };
  // Custom elements, upgraded in the light DOM. A definition upgrades every
  // element already in the tree with that name and everything inserted or
  // created later: the class's constructor runs with the element as `this`,
  // then `connectedCallback`, then `attributeChangedCallback` for each
  // observed attribute present. Shadow trees a component renders into are
  // invisible to the reader either way; the light DOM it fills in is not.
  const definitions = new Map();
  const whenDefined = new Map();
  const upgrading = [];
  const upgraded = new Map();
  const upgradeOne = (el, definition) => {
    const key = keyOf(el);
    if (upgraded.has(key)) return;
    upgraded.set(key, definition);
    upgrading.push(el);
    const before = Object.getPrototypeOf(el);
    try {
      new definition.ctor();
      // `super()` handed the constructor this very element and put it on
      // `new.target.prototype`. A constructor that swapped the prototype
      // itself since (an ES5 shim does, to the transpiled class's) keeps
      // what it chose; one that returned some other object left the element
      // on its built-in prototype, and the class's is where its methods
      // live.
      if (Object.getPrototypeOf(el) === before) Object.setPrototypeOf(el, definition.ctor.prototype);
    } catch (e) {
      native.record_error('customElements:' + definition.name, describeError(e));
      upgrading.splice(upgrading.indexOf(el), 1);
      return;
    }
    if (upgrading.length && keyOf(upgrading[upgrading.length - 1]) === key) upgrading.pop();
    const observed = definition.observed;
    if (observed.length && typeof el.attributeChangedCallback === 'function') {
      for (const name of observed) {
        if (el.hasAttribute(name)) {
          try { el.attributeChangedCallback(name, null, el.getAttribute(name), null); }
          catch (e) { native.record_error('attributeChangedCallback:' + definition.name, describeError(e)); }
        }
      }
    }
    if (el.isConnected && typeof el.connectedCallback === 'function') {
      try { el.connectedCallback(); }
      catch (e) { native.record_error('connectedCallback:' + definition.name, describeError(e)); }
    }
  };
  const upgradeWithin = (root, inserted = false) => {
    if (!root || (root.nodeType !== 1 && root.nodeType !== 11) || definitions.size === 0) return;
    const candidates = root.nodeType === 1 && root.tagName.includes('-') ? [root] : [];
    for (const el of root.querySelectorAll('*')) if (el.tagName.includes('-')) candidates.push(el);
    for (const el of candidates) {
      const definition = definitions.get(el.tagName.toLowerCase());
      if (!definition) continue;
      if (upgraded.has(keyOf(el))) {
        // Already a component; being put into the tree is what it wants
        // to hear about.
        if (inserted && el.isConnected && typeof el.connectedCallback === 'function') {
          try { el.connectedCallback(); }
          catch (e) { native.record_error('connectedCallback:' + definition.name, describeError(e)); }
        }
        continue;
      }
      upgradeOne(el, definition);
    }
  };
  globalThis.__mar_upgrade_within = upgradeWithin;
  globalThis.customElements = {
    define(name, ctor, options = {}) {
      name = String(name).toLowerCase();
      if (definitions.has(name)) throw new globalThis.DOMException(`'${name}' has already been defined`, 'NotSupportedError');
      if (typeof ctor !== 'function') throw new TypeError('constructor is not a function');
      let observed = [];
      try { observed = Array.from(ctor.observedAttributes || []).map(String); } catch (e) { /* not observable */ }
      const definition = { name, ctor, observed, extends: options.extends ?? null };
      definitions.set(name, definition);
      const waiting = whenDefined.get(name);
      if (waiting) { whenDefined.delete(name); waiting.resolve(ctor); }
      upgradeWithin(document.documentElement);
    },
    get(name) { return definitions.get(String(name).toLowerCase())?.ctor; },
    getName(ctor) { for (const [name, d] of definitions) if (d.ctor === ctor) return name; return null; },
    whenDefined(name) {
      name = String(name).toLowerCase();
      const known = definitions.get(name);
      if (known) return Promise.resolve(known.ctor);
      let entry = whenDefined.get(name);
      if (!entry) {
        entry = {};
        entry.promise = new Promise((resolve) => { entry.resolve = resolve; });
        whenDefined.set(name, entry);
      }
      return entry.promise;
    },
    upgrade(root) { upgradeWithin(root); },
  };
  // An element being upgraded is what its class's constructor gets as
  // `this`: `super()` in `class X extends HTMLElement` lands here.
  globalThis.__mar_element_under_construction = () => upgrading.length ? upgrading[upgrading.length - 1] : null;

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
  const DOM_EXCEPTION_CODES = {
    IndexSizeError: 1, HierarchyRequestError: 3, WrongDocumentError: 4, InvalidCharacterError: 5,
    NoModificationAllowedError: 7, NotFoundError: 8, NotSupportedError: 9, InUseAttributeError: 10,
    InvalidStateError: 11, SyntaxError: 12, InvalidModificationError: 13, NamespaceError: 14,
    InvalidAccessError: 15, TypeMismatchError: 17, SecurityError: 18, NetworkError: 19,
    AbortError: 20, URLMismatchError: 21, QuotaExceededError: 22, TimeoutError: 23,
    InvalidNodeTypeError: 24, DataCloneError: 25,
  };
  globalThis.DOMException = class DOMException extends Error {
    constructor(message = '', name = 'Error') {
      super(String(message));
      // Own data properties, not assignments: core-js installs accessors for
      // `name` and `code` on the prototype before it constructs one of these
      // to probe it, and an assignment through a getter-only accessor throws.
      define(this, 'name', { value: String(name), writable: true });
      define(this, 'code', { value: DOM_EXCEPTION_CODES[String(name)] ?? 0, writable: true });
    }
  };
  for (const [name, code] of Object.entries({
    INDEX_SIZE_ERR: 1, DOMSTRING_SIZE_ERR: 2, HIERARCHY_REQUEST_ERR: 3, WRONG_DOCUMENT_ERR: 4,
    INVALID_CHARACTER_ERR: 5, NO_DATA_ALLOWED_ERR: 6, NO_MODIFICATION_ALLOWED_ERR: 7,
    NOT_FOUND_ERR: 8, NOT_SUPPORTED_ERR: 9, INUSE_ATTRIBUTE_ERR: 10, INVALID_STATE_ERR: 11,
    SYNTAX_ERR: 12, INVALID_MODIFICATION_ERR: 13, NAMESPACE_ERR: 14, INVALID_ACCESS_ERR: 15,
    VALIDATION_ERR: 16, TYPE_MISMATCH_ERR: 17, SECURITY_ERR: 18, NETWORK_ERR: 19, ABORT_ERR: 20,
    URL_MISMATCH_ERR: 21, QUOTA_EXCEEDED_ERR: 22, TIMEOUT_ERR: 23, INVALID_NODE_TYPE_ERR: 24,
    DATA_CLONE_ERR: 25,
  })) {
    define(globalThis.DOMException, name, { value: code });
    define(globalThis.DOMException.prototype, name, { value: code });
  }

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
        catch (e) { native.record_error('abort listener', describeError(e)); }
      }
      if (typeof this.onabort === 'function') {
        try { this.onabort(event); }
        catch (e) { native.record_error('onabort', describeError(e)); }
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
          catch (e) { native.record_error('MessagePort', describeError(e)); }
        }
        if (typeof this.onmessage === 'function') {
          try { this.onmessage(event); }
          catch (e) { native.record_error('MessagePort', describeError(e)); }
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
    method(DocumentProto, 'createNodeIterator', (root, show, filter) => new NodeIterator(root, show, filter));
    method(DocumentProto, 'createTreeWalker', (root, show, filter) => new globalThis.TreeWalker(root, show, filter));

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
    method(DocumentProto, 'createRange', () => new globalThis.Range());
    named('Selection', class Selection {
      constructor() { this.rangeCount = 0; this.isCollapsed = true; this.type = 'None'; }
      getRangeAt() { throw new globalThis.DOMException('no range', 'IndexSizeError'); }
      addRange() {} removeAllRanges() {} removeRange() {} collapse() {}
      selectAllChildren() {} toString() { return ''; }
    });
    const selection = new globalThis.Selection();
    globalThis.getSelection = () => selection;
    method(DocumentProto, 'getSelection', () => selection);

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


  // -- what the corpus asked for ---------------------------------------------

  // Each of these is a name some real page called and did not find. Most
  // are a shape and a plausible answer, because a plausible answer is what
  // lets the page get on with rendering; none of them draws, lays out or
  // reaches a device, because nothing here can.

  // A canvas that answers. Lottie, web-animations and a fingerprinting
  // script all open a 2D context at import time and set a property on it,
  // so `null` — which is what a headless machine without a GPU may return
  // for WebGL — is not enough for '2d'.
  const canvasContexts = new Map();
  const noop = () => {};
  const imageData = (w, h) => ({
    width: w | 0, height: h | 0, colorSpace: 'srgb',
    data: new Uint8ClampedArray(Math.max(0, (w | 0) * (h | 0) * 4)),
  });
  const identityMatrix = () => ({
    a: 1, b: 0, c: 0, d: 1, e: 0, f: 0, m11: 1, m12: 0, m21: 0, m22: 1, m41: 0, m42: 0,
    is2D: true, isIdentity: true,
  });
  const gradient = () => ({ addColorStop: noop });
  const context2d = (canvas) => {
    const ctx = {
      canvas,
      fillStyle: '#000000', strokeStyle: '#000000', lineWidth: 1, lineCap: 'butt',
      lineJoin: 'miter', miterLimit: 10, lineDashOffset: 0, font: '10px sans-serif',
      textAlign: 'start', textBaseline: 'alphabetic', direction: 'ltr', letterSpacing: '0px',
      wordSpacing: '0px', fontKerning: 'auto', fontStretch: 'normal', fontVariantCaps: 'normal',
      textRendering: 'auto', globalAlpha: 1, globalCompositeOperation: 'source-over',
      imageSmoothingEnabled: true, imageSmoothingQuality: 'low', filter: 'none',
      shadowBlur: 0, shadowColor: 'rgba(0, 0, 0, 0)', shadowOffsetX: 0, shadowOffsetY: 0,
      measureText: (text) => {
        const width = String(text).length * 6;
        return {
          width, actualBoundingBoxLeft: 0, actualBoundingBoxRight: width,
          actualBoundingBoxAscent: 8, actualBoundingBoxDescent: 2,
          fontBoundingBoxAscent: 8, fontBoundingBoxDescent: 2,
          emHeightAscent: 8, emHeightDescent: 2, hangingBaseline: 8,
          alphabeticBaseline: 0, ideographicBaseline: -2,
        };
      },
      getImageData: (_x, _y, w, h) => imageData(w, h),
      createImageData: (w, h) => imageData(typeof w === 'object' ? w.width : w, typeof w === 'object' ? w.height : h),
      createLinearGradient: gradient, createRadialGradient: gradient, createConicGradient: gradient,
      createPattern: () => null,
      getContextAttributes: () => ({ alpha: true, colorSpace: 'srgb', desynchronized: false, willReadFrequently: false }),
      getTransform: identityMatrix, getLineDash: () => [],
      isPointInPath: () => false, isPointInStroke: () => false, isContextLost: () => false,
    };
    for (const name of [
      'fillRect', 'strokeRect', 'clearRect', 'beginPath', 'closePath', 'moveTo', 'lineTo',
      'bezierCurveTo', 'quadraticCurveTo', 'arc', 'arcTo', 'ellipse', 'rect', 'roundRect',
      'fill', 'stroke', 'clip', 'fillText', 'strokeText', 'drawImage', 'putImageData',
      'save', 'restore', 'scale', 'rotate', 'translate', 'transform', 'setTransform',
      'resetTransform', 'setLineDash', 'drawFocusIfNeeded', 'reset',
    ]) ctx[name] = noop;
    return ctx;
  };
  method(NodeProto, 'getContext', function (kind) {
    if (this.tagName !== 'CANVAS') return null;
    if (String(kind) !== '2d') return null;
    const key = keyOf(this);
    if (!canvasContexts.has(key)) canvasContexts.set(key, context2d(this));
    return canvasContexts.get(key);
  });
  // One transparent pixel, which is what an unpainted canvas holds.
  const BLANK_PNG = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==';
  method(NodeProto, 'toDataURL', () => BLANK_PNG);
  method(NodeProto, 'toBlob', (callback) => {
    setTimeout(() => { if (typeof callback === 'function') callback(new Blob([''], { type: 'image/png' })); }, 0);
  });
  method(NodeProto, 'captureStream', () => ({ getTracks: () => [], getVideoTracks: () => [], getAudioTracks: () => [], addTrack: noop, removeTrack: noop }));
  globalThis.CanvasRenderingContext2D = class CanvasRenderingContext2D {};
  globalThis.Path2D = class Path2D {
    addPath() {} closePath() {} moveTo() {} lineTo() {} bezierCurveTo() {} quadraticCurveTo() {}
    arc() {} arcTo() {} ellipse() {} rect() {} roundRect() {}
  };
  globalThis.DOMMatrixReadOnly = class DOMMatrixReadOnly {
    constructor() { Object.assign(this, identityMatrix()); }
    multiply() { return new globalThis.DOMMatrix(); }
    translate() { return new globalThis.DOMMatrix(); }
    scale() { return new globalThis.DOMMatrix(); }
    inverse() { return new globalThis.DOMMatrix(); }
    toString() { return 'matrix(1, 0, 0, 1, 0, 0)'; }
    toJSON() { return { ...this }; }
  };
  globalThis.DOMMatrix = class DOMMatrix extends globalThis.DOMMatrixReadOnly {};
  globalThis.WebKitCSSMatrix = globalThis.DOMMatrix;

  // The size of a picture, canvas or frame. `canvas.width = 1` is how a page
  // sizes the canvas it is about to measure with.
  const SIZED_NUMERIC = ['IMG', 'CANVAS', 'VIDEO', 'INPUT'];
  const SIZED_STRING = ['IFRAME', 'EMBED', 'OBJECT', 'TABLE', 'TD', 'TH', 'COL', 'COLGROUP', 'HR', 'PRE'];
  for (const prop of ['width', 'height']) {
    define(NodeProto, prop, {
      get() {
        if (SIZED_NUMERIC.includes(this.tagName)) {
          const raw = parseInt(this.getAttribute(prop) ?? '', 10);
          if (!isNaN(raw)) return raw;
          return this.tagName === 'CANVAS' ? (prop === 'width' ? 300 : 150) : 0;
        }
        if (SIZED_STRING.includes(this.tagName)) return this.getAttribute(prop) ?? '';
        return undefined;
      },
      set(v) { this.setAttribute(prop, String(v)); },
    });
  }
  define(NodeProto, 'naturalWidth', { get() { return this.tagName === 'IMG' ? this.width : undefined; } });
  define(NodeProto, 'naturalHeight', { get() { return this.tagName === 'IMG' ? this.height : undefined; } });
  define(NodeProto, 'complete', { get() { return this.tagName === 'IMG' ? true : undefined; } });
  define(NodeProto, 'currentSrc', {
    get() { return ['IMG', 'VIDEO', 'AUDIO'].includes(this.tagName) ? this.src : undefined; },
  });
  method(NodeProto, 'decode', () => Promise.resolve());
  // Media that never plays. The promise from `play()` resolves, because a
  // rejection is what a page treats as "autoplay was blocked" and reacts to.
  method(NodeProto, 'canPlayType', () => '');
  method(NodeProto, 'play', () => Promise.resolve());
  method(NodeProto, 'pause', noop);
  method(NodeProto, 'load', noop);
  method(NodeProto, 'requestPictureInPicture', () => Promise.reject(new globalThis.DOMException('no picture', 'NotSupportedError')));
  method(NodeProto, 'requestFullscreen', () => Promise.reject(new globalThis.DOMException('no fullscreen', 'NotSupportedError')));
  for (const [prop, value] of [
    ['paused', true], ['ended', false], ['muted', false], ['volume', 1], ['currentTime', 0],
    ['duration', NaN], ['playbackRate', 1], ['videoWidth', 0], ['videoHeight', 0],
    ['networkState', 0], ['seeking', false], ['autoplay', false], ['loop', false],
    ['controls', false], ['playsInline', false], ['defaultMuted', false],
  ]) {
    define(NodeProto, prop, {
      get() { return ['VIDEO', 'AUDIO'].includes(this.tagName) ? value : undefined; },
      set: noop,
    });
  }
  define(NodeProto, 'buffered', { get: () => ({ length: 0, start: () => 0, end: () => 0 }) });
  define(NodeProto, 'played', { get: () => ({ length: 0, start: () => 0, end: () => 0 }) });
  define(NodeProto, 'seekable', { get: () => ({ length: 0, start: () => 0, end: () => 0 }) });
  define(NodeProto, 'textTracks', { get: () => [] });

  // A frame with a window in it. Akamai's mPulse snippet, tag managers and
  // ad loaders all open one, write into its document and expect the script
  // they wrote to load there; nothing loads, but the document they write
  // into exists, so the writer does not throw.
  const frameWindows = new Map();
  const frameWindow = (iframe) => {
    const key = keyOf(iframe);
    if (frameWindows.has(key)) return frameWindows.get(key);
    const doc = document.implementation.createHTMLDocument('');
    define(doc, 'defaultView', { get: () => win, configurable: true });
    const win = {
      frameElement: iframe, parent: globalThis, top: globalThis, opener: null,
      navigator: globalThis.navigator, screen: globalThis.screen, history: globalThis.history,
      localStorage: globalThis.localStorage, sessionStorage: globalThis.sessionStorage,
      performance: globalThis.performance, console: globalThis.console, name: '',
      closed: false, length: 0, innerWidth: 0, innerHeight: 0, outerWidth: 0, outerHeight: 0,
      devicePixelRatio: 1, scrollX: 0, scrollY: 0, pageXOffset: 0, pageYOffset: 0,
      location: {
        href: 'about:blank', protocol: 'about:', host: '', hostname: '', port: '',
        pathname: 'blank', search: '', hash: '', origin: 'null',
        assign: noop, replace: noop, reload: noop, toString: () => 'about:blank',
      },
      postMessage: noop, addEventListener: noop, removeEventListener: noop,
      dispatchEvent: () => true, focus: noop, blur: noop, close: noop, open: () => null,
      alert: noop, confirm: () => false, prompt: () => null, print: noop, stop: noop,
      scrollTo: noop, scrollBy: noop, scroll: noop,
      setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout,
      setInterval: globalThis.setInterval, clearInterval: globalThis.clearInterval,
      requestAnimationFrame: globalThis.requestAnimationFrame,
      cancelAnimationFrame: globalThis.cancelAnimationFrame,
      requestIdleCallback: globalThis.requestIdleCallback, queueMicrotask: globalThis.queueMicrotask,
      getComputedStyle: globalThis.getComputedStyle, matchMedia: globalThis.matchMedia,
      eval: (source) => globalThis.eval(String(source)),
      fetch: globalThis.fetch, XMLHttpRequest: globalThis.XMLHttpRequest, URL: globalThis.URL,
      Blob: globalThis.Blob, Image: globalThis.Image, Event: globalThis.Event,
      CustomEvent: globalThis.CustomEvent, Node: globalThis.Node, Element: globalThis.Element,
      HTMLElement: globalThis.HTMLElement, Promise, Object, Array, Function, String, Number,
      Boolean, Date, RegExp, Error, TypeError, RangeError, JSON, Math, Symbol, Map, Set, WeakMap,
      WeakSet, Proxy, Reflect, ArrayBuffer, DataView, Uint8Array, Int32Array, Float64Array,
      parseInt, parseFloat, isNaN, isFinite, encodeURIComponent, decodeURIComponent,
      encodeURI, decodeURI, btoa: globalThis.btoa, atob: globalThis.atob,
    };
    win.self = win; win.window = win; win.globalThis = win; win.frames = win;
    win.document = doc;
    define(doc, 'domain', { get: () => location.hostname, set: noop });
    define(doc, 'location', { get: () => win.location });
    frameWindows.set(key, win);
    return win;
  };
  define(NodeProto, 'contentWindow', {
    get() { return this.tagName === 'IFRAME' ? frameWindow(this) : undefined; },
  });
  define(NodeProto, 'contentDocument', {
    get() { return this.tagName === 'IFRAME' ? frameWindow(this).document : undefined; },
  });
  method(NodeProto, 'getSVGDocument', () => null);

  // A stylesheet object per <style> and per stylesheet <link>: anchor-js and
  // its relatives insert their rules through it rather than by writing text.
  const sheets = new Map();
  define(NodeProto, 'sheet', {
    get() {
      const isSheet = this.tagName === 'STYLE'
        || (this.tagName === 'LINK' && /(^|\s)stylesheet(\s|$)/i.test(this.getAttribute('rel') ?? ''));
      if (!isSheet) return undefined;
      const key = keyOf(this);
      if (!sheets.has(key)) {
        const sheet = new globalThis.CSSStyleSheet();
        sheet.ownerNode = this;
        sheet.href = this.tagName === 'LINK' ? this.href : null;
        sheet.media = { mediaText: this.getAttribute('media') ?? '', length: 0, item: () => null, appendMedium: noop, deleteMedium: noop };
        sheet.title = this.getAttribute('title');
        sheet.type = 'text/css';
        sheet.parentStyleSheet = null;
        if (this.tagName === 'STYLE') sheet.replaceSync(this.textContent);
        sheets.set(key, sheet);
      }
      return sheets.get(key);
    },
  });

  // Forms and their controls.
  define(NodeProto, 'elements', {
    get() {
      if (this.tagName !== 'FORM' && this.tagName !== 'FIELDSET') return undefined;
      const list = this.querySelectorAll('input, select, textarea, button, fieldset, output, object');
      list.namedItem = (n) => list.find((e) => e.getAttribute('name') === n || e.id === n) ?? null;
      for (const el of list) {
        const name = el.getAttribute('name');
        if (name && !(name in list)) list[name] = el;
      }
      return list;
    },
  });
  define(NodeProto, 'options', {
    get() {
      if (this.tagName !== 'SELECT' && this.tagName !== 'DATALIST') return undefined;
      const list = this.querySelectorAll('option');
      list.item = (i) => list[i] ?? null;
      list.namedItem = (n) => list.find((o) => o.id === n || o.getAttribute('name') === n) ?? null;
      list.selectedIndex = list.findIndex((o) => o.hasAttribute('selected'));
      return list;
    },
  });
  define(NodeProto, 'selectedOptions', {
    get() { return this.tagName === 'SELECT' ? this.querySelectorAll('option[selected]') : undefined; },
  });
  define(NodeProto, 'selectedIndex', {
    get() {
      if (this.tagName !== 'SELECT') return undefined;
      const options = this.querySelectorAll('option');
      const chosen = options.findIndex((o) => o.hasAttribute('selected'));
      return chosen >= 0 ? chosen : (options.length ? 0 : -1);
    },
    set(index) {
      if (this.tagName !== 'SELECT') return;
      this.querySelectorAll('option').forEach((o, i) => {
        if (i === Number(index)) o.setAttribute('selected', ''); else o.removeAttribute('selected');
      });
    },
  });
  define(NodeProto, 'multiple', {
    get() { return this.hasAttribute('multiple'); },
    set(v) { if (v) this.setAttribute('multiple', ''); else this.removeAttribute('multiple'); },
  });
  define(NodeProto, 'form', {
    get() { return this.nodeType === 1 ? (this.closest('form') ?? null) : null; },
  });
  define(NodeProto, 'labels', { get() { return this.id ? document.querySelectorAll(`label[for="${this.id}"]`) : []; } });
  define(NodeProto, 'files', { get() { return this.tagName === 'INPUT' ? [] : undefined; } });
  define(NodeProto, 'validity', {
    get: () => ({ valid: true, valueMissing: false, typeMismatch: false, patternMismatch: false,
      tooLong: false, tooShort: false, rangeUnderflow: false, rangeOverflow: false,
      stepMismatch: false, badInput: false, customError: false }),
  });
  define(NodeProto, 'validationMessage', { get: () => '' });
  define(NodeProto, 'willValidate', { get() { return ['INPUT', 'SELECT', 'TEXTAREA', 'BUTTON'].includes(this.tagName); } });
  method(NodeProto, 'checkValidity', () => true);
  method(NodeProto, 'reportValidity', () => true);
  method(NodeProto, 'setCustomValidity', noop);
  method(NodeProto, 'select', noop);
  method(NodeProto, 'setSelectionRange', noop);
  method(NodeProto, 'setRangeText', noop);
  method(NodeProto, 'stepUp', noop);
  method(NodeProto, 'stepDown', noop);
  method(NodeProto, 'reset', function () {
    dispatchEvent.call(this, new Event('reset', { bubbles: true, cancelable: true }));
  });
  define(NodeProto, 'valueAsNumber', {
    get() { const n = parseFloat(this.value); return isNaN(n) ? NaN : n; },
    set(v) { this.value = String(v); },
  });
  define(NodeProto, 'defaultValue', {
    get() { return this.getAttribute('value') ?? ''; },
    set(v) { this.setAttribute('value', String(v)); },
  });
  define(NodeProto, 'defaultChecked', {
    get() { return this.hasAttribute('checked'); },
    set(v) { if (v) this.setAttribute('checked', ''); else this.removeAttribute('checked'); },
  });
  define(NodeProto, 'indeterminate', { get: () => false, set: noop });
  define(NodeProto, 'selectionStart', { get: () => 0, set: noop });
  define(NodeProto, 'selectionEnd', { get: () => 0, set: noop });
  define(NodeProto, 'selectionDirection', { get: () => 'none', set: noop });
  define(NodeProto, 'maxLength', {
    get() { const n = parseInt(this.getAttribute('maxlength') ?? '', 10); return isNaN(n) ? -1 : n; },
    set(v) { this.setAttribute('maxlength', String(v | 0)); },
  });
  define(NodeProto, 'minLength', {
    get() { const n = parseInt(this.getAttribute('minlength') ?? '', 10); return isNaN(n) ? -1 : n; },
    set(v) { this.setAttribute('minlength', String(v | 0)); },
  });
  define(NodeProto, 'size', {
    get() { const n = parseInt(this.getAttribute('size') ?? '', 10); return isNaN(n) ? 20 : n; },
    set(v) { this.setAttribute('size', String(v | 0)); },
  });
  define(NodeProto, 'rows', {
    get() { const n = parseInt(this.getAttribute('rows') ?? '', 10); return isNaN(n) ? 2 : n; },
    set(v) { this.setAttribute('rows', String(v | 0)); },
  });
  define(NodeProto, 'cols', {
    get() { const n = parseInt(this.getAttribute('cols') ?? '', 10); return isNaN(n) ? 20 : n; },
    set(v) { this.setAttribute('cols', String(v | 0)); },
  });
  define(NodeProto, 'open', {
    get() { return this.hasAttribute('open'); },
    set(v) { if (v) this.setAttribute('open', ''); else this.removeAttribute('open'); },
  });
  method(NodeProto, 'showModal', function () { this.setAttribute('open', ''); });
  method(NodeProto, 'show', function () { this.setAttribute('open', ''); });
  method(NodeProto, 'close', function () { this.removeAttribute('open'); });
  method(NodeProto, 'showPopover', noop);
  method(NodeProto, 'hidePopover', noop);
  method(NodeProto, 'togglePopover', () => false);

  // Focus order, visibility and the properties around them.
  const FOCUSABLE = /^(A|AREA|BUTTON|INPUT|SELECT|TEXTAREA|IFRAME|SUMMARY|DETAILS)$/;
  define(NodeProto, 'tabIndex', {
    get() {
      const raw = parseInt(this.getAttribute('tabindex') ?? '', 10);
      if (!isNaN(raw)) return raw;
      return FOCUSABLE.test(this.tagName) ? 0 : -1;
    },
    set(v) { this.setAttribute('tabindex', String(v | 0)); },
  });
  method(NodeProto, 'checkVisibility', function () {
    if (this.nodeType !== 1) return false;
    const style = cascade.for(this);
    return style.display !== 'none' && style.visibility !== 'hidden' && !this.hasAttribute('hidden');
  });
  define(NodeProto, 'inert', {
    get() { return this.hasAttribute('inert'); },
    set(v) { if (v) this.setAttribute('inert', ''); else this.removeAttribute('inert'); },
  });
  define(NodeProto, 'autofocus', {
    get() { return this.hasAttribute('autofocus'); },
    set(v) { if (v) this.setAttribute('autofocus', ''); else this.removeAttribute('autofocus'); },
  });
  define(NodeProto, 'draggable', {
    get() { return this.getAttribute('draggable') === 'true' || this.tagName === 'A' || this.tagName === 'IMG'; },
    set(v) { this.setAttribute('draggable', v ? 'true' : 'false'); },
  });
  define(NodeProto, 'spellcheck', {
    get() { return this.getAttribute('spellcheck') !== 'false'; },
    set(v) { this.setAttribute('spellcheck', v ? 'true' : 'false'); },
  });
  define(NodeProto, 'translate', {
    get() { return this.getAttribute('translate') !== 'no'; },
    set(v) { this.setAttribute('translate', v ? 'yes' : 'no'); },
  });
  define(NodeProto, 'contentEditable', {
    get() { return this.getAttribute('contenteditable') ?? 'inherit'; },
    set(v) { this.setAttribute('contenteditable', String(v)); },
  });
  define(NodeProto, 'isContentEditable', {
    get() { return this.nodeType === 1 && !!this.closest('[contenteditable=""], [contenteditable="true"]'); },
  });
  // A shadow root is a fragment attached to a host. Its content is not the
  // page's content — a screen reader announcer in a shadow root is exactly
  // what a reader should not read — but Next.js attaches one to every page
  // it routes, and without the method the router never finishes mounting.
  const shadowRoots = new Map();
  method(NodeProto, 'attachShadow', function (init = {}) {
    if (this.nodeType !== 1) throw new globalThis.DOMException('not an element', 'NotSupportedError');
    const key = keyOf(this);
    if (shadowRoots.has(key)) throw new globalThis.DOMException('Shadow root cannot be created on a host which already hosts a shadow tree', 'NotSupportedError');
    const root = native.create_fragment();
    Object.setPrototypeOf(root, ShadowRoot.prototype);
    shadowRoots.set(key, root);
    shadowHosts.set(keyOf(root), this);
    define(root, 'mode', { value: init.mode === 'closed' ? 'closed' : 'open' });
    define(root, 'delegatesFocus', { value: !!init.delegatesFocus });
    define(root, 'slotAssignment', { value: init.slotAssignment ?? 'named' });
    define(root, 'adoptedStyleSheets', { value: [], writable: true });
    return root;
  });
  define(ShadowRoot.prototype, 'host', { get() { return shadowHosts.get(keyOf(this)) ?? null; } });
  method(ShadowRoot.prototype, 'getElementById', function (id) {
    return this.querySelector(`[id="${String(id).replace(/"/g, '\\"')}"]`);
  });
  define(NodeProto, 'shadowRoot', {
    get() { const root = shadowRoots.get(keyOf(this)); return root && root.mode === 'open' ? root : null; },
  });
  define(NodeProto, 'assignedSlot', { get: () => null });
  define(NodeProto, 'part', { get() { return new DOMTokenList(this); } });
  define(NodeProto, 'elementTiming', { get: () => '' });
  method(NodeProto, 'getAnimations', () => []);
  method(NodeProto, 'attributeStyleMap', () => ({}));

  // Animations that are already over. A component that awaits `finished`
  // before revealing its content gets to reveal it.
  class Animation {
    constructor(effect = null, timeline = null) {
      this.effect = effect; this.timeline = timeline; this.id = '';
      this.playState = 'finished'; this.playbackRate = 1; this.startTime = 0;
      this.currentTime = 0; this.pending = false; this.replaceState = 'active';
      this.onfinish = null; this.oncancel = null; this.onremove = null;
      this.ready = Promise.resolve(this);
      this.finished = Promise.resolve(this);
      setTimeout(() => { if (typeof this.onfinish === 'function') this.onfinish(new Event('finish')); }, 0);
    }
    play() {} pause() {} cancel() {} finish() {} reverse() {} commitStyles() {} persist() {}
    updatePlaybackRate() {} addEventListener() {} removeEventListener() {} dispatchEvent() { return true; }
  }
  globalThis.Animation = Animation;
  globalThis.KeyframeEffect = class KeyframeEffect {
    constructor(target, keyframes, options) { this.target = target; this._keyframes = keyframes; this._options = options; }
    getKeyframes() { return Array.isArray(this._keyframes) ? this._keyframes : []; }
    setKeyframes() {} getTiming() { return { duration: 0, delay: 0, iterations: 1 }; } getComputedTiming() { return this.getTiming(); }
  };
  globalThis.AnimationEffect = globalThis.KeyframeEffect;
  globalThis.DocumentTimeline = class DocumentTimeline { get currentTime() { return native.now(); } };
  method(NodeProto, 'animate', function (keyframes, options) {
    return new Animation(new globalThis.KeyframeEffect(this, keyframes, options), null);
  });

  // A script the page inserts runs, the way it does in a browser. Webpack
  // loads every lazy chunk this way, a tag manager loads everything this
  // way, and a page whose inserted scripts never ran is a page whose
  // application never started. Scripts the parser saw have already run;
  // scripts that arrived through innerHTML never run, as the spec says.
  const startedScripts = new Set();
  const markStarted = (root) => {
    if (!root || (root.nodeType !== 1 && root.nodeType !== 11)) return;
    if (root.tagName === 'SCRIPT') startedScripts.add(keyOf(root));
    for (const s of root.querySelectorAll('script')) startedScripts.add(keyOf(s));
  };
  markStarted(document.documentElement);
  const JS_TYPES = /^(text|application)\/(x-)?(java|ecma)script$|^module$|^text\/jsx?$/i;
  const fireScriptEvent = (el, type) => {
    try { fireOn(el, Object.assign(new Event(type), { target: el })); }
    catch (e) { native.record_error('script:' + type, describeError(e)); }
  };
  const runModuleSource = (source, origin) => {
    let promise;
    try { promise = native.run_module(String(source), String(origin)); }
    catch (e) { native.record_error(origin, 'module compile failed: ' + describeError(e)); return; }
    Promise.resolve(promise).catch((e) => native.record_error(origin, describeError(e)));
  };
  const startScript = (el) => {
    startedScripts.add(keyOf(el));
    const type = (el.getAttribute('type') || '').trim();
    if (type && !JS_TYPES.test(type)) return;
    if (el.hasAttribute('nomodule')) return;
    const isModule = /^module$/i.test(type);
    const src = el.getAttribute('src');
    if (src != null && src.trim() !== '') {
      let url;
      try { url = new URL(src, location.href).href; } catch { fireScriptEvent(el, 'error'); return; }
      requestAsync('GET', url, { Accept: '*/*' }, null).then((raw) => {
        if (!raw || !raw.ok) { fireScriptEvent(el, 'error'); return; }
        if (isModule) runModuleSource(raw.body, raw.url || url);
        else native.run_script(String(raw.body ?? ''), raw.url || url, keyOf(el));
        fireScriptEvent(el, 'load');
      });
      return;
    }
    const body = el.textContent;
    if (!body.trim()) return;
    if (isModule) runModuleSource(body, location.href);
    else native.run_script(body, `inline script #${keyOf(el)}`, keyOf(el));
  };
  const scriptsWithin = (node) => {
    if (!node || typeof node !== 'object' || (node.nodeType !== 1 && node.nodeType !== 11)) return [];
    const found = node.nodeType === 1 && node.tagName === 'SCRIPT' ? [node] : [];
    for (const s of node.querySelectorAll('script')) found.push(s);
    return found;
  };
  const afterInsert = (candidates) => {
    for (const s of candidates) {
      if (!startedScripts.has(keyOf(s)) && s.isConnected) startScript(s);
    }
  };
  const hookInsert = (name, pick) => {
    const original = NodeProto[name];
    method(NodeProto, name, function (...args) {
      const inserted = pick(args);
      const candidates = inserted.flatMap(scriptsWithin);
      const result = original.apply(this, args);
      if (candidates.length) afterInsert(candidates);
      if (this.isConnected) for (const node of inserted) upgradeWithin(node, true);
      return result;
    });
  };
  hookInsert('appendChild', (args) => [args[0]]);
  hookInsert('insertBefore', (args) => [args[0]]);
  hookInsert('replaceChild', (args) => [args[0]]);
  hookInsert('append', (args) => args);
  hookInsert('prepend', (args) => args);
  // Markup that arrives as text brings scripts that must not run.
  const markupSetter = (name) => {
    const original = Object.getOwnPropertyDescriptor(NodeProto, name);
    define(NodeProto, name, {
      get: original.get,
      set(v) {
        original.set.call(this, v);
        const root = name === 'outerHTML' ? this.parentNode : this;
        markStarted(root);
        if (root && root.isConnected) upgradeWithin(root);
      },
    });
  };
  markupSetter('innerHTML');
  markupSetter('outerHTML');
  const nativeInsertAdjacentHTML = NodeProto.insertAdjacentHTML;
  method(NodeProto, 'insertAdjacentHTML', function (position, html) {
    const result = nativeInsertAdjacentHTML.call(this, position, html);
    markStarted(this.parentNode || this);
    return result;
  });
  // `s.src = url` on a script already in the tree starts it too.
  const srcReflect = Object.getOwnPropertyDescriptor(NodeProto, 'src');
  define(NodeProto, 'src', {
    get: srcReflect.get,
    set(v) {
      srcReflect.set.call(this, v);
      if (this.tagName === 'SCRIPT' && this.isConnected && !startedScripts.has(keyOf(this))) startScript(this);
    },
  });
  globalThis.HTMLScriptElement.supports = (type) => type === 'classic' || type === 'module' || type === 'importmap';

  // A submitted form navigates. `submit()` goes without an event, as the
  // spec has it; `requestSubmit()` asks the page first.
  const formData = (form, submitter) => {
    const pairs = [];
    for (const el of form.querySelectorAll('input, select, textarea, button')) {
      const name = el.getAttribute('name');
      if (!name || el.hasAttribute('disabled')) continue;
      const tag = el.tagName;
      const type = (el.getAttribute('type') || 'text').toLowerCase();
      if (tag === 'BUTTON' || (tag === 'INPUT' && (type === 'submit' || type === 'image' || type === 'button' || type === 'reset'))) {
        if (submitter && keyOf(submitter) === keyOf(el)) pairs.push([name, el.value]);
        continue;
      }
      if (tag === 'INPUT' && (type === 'checkbox' || type === 'radio') && !el.checked) continue;
      if (tag === 'INPUT' && type === 'file') continue;
      if (tag === 'SELECT') {
        for (const o of el.querySelectorAll('option[selected]')) pairs.push([name, o.value]);
        if (!el.querySelector('option[selected]') && !el.hasAttribute('multiple')) {
          const first = el.querySelector('option');
          if (first) pairs.push([name, first.value]);
        }
        continue;
      }
      pairs.push([name, el.value]);
    }
    return pairs;
  };
  const submitForm = (form, submitter) => {
    const method = String(submitter?.getAttribute('formmethod') || form.getAttribute('method') || 'get').toLowerCase();
    const action = submitter?.getAttribute('formaction') || form.getAttribute('action') || location.href;
    let url;
    try { url = new URL(action, location.href); } catch { return; }
    const body = new URLSearchParams(formData(form, submitter)).toString();
    if (method === 'post') {
      native.navigate(url.href, 'POST', body);
    } else {
      url.search = body ? '?' + body : '';
      native.navigate(url.href);
    }
  };
  method(NodeProto, 'submit', function () {
    if (this.tagName === 'FORM') submitForm(this, null);
  });
  method(NodeProto, 'requestSubmit', function (submitter) {
    if (this.tagName !== 'FORM') return;
    const event = new Event('submit', { bubbles: true, cancelable: true });
    event.submitter = submitter ?? null;
    if (dispatchEvent.call(this, event)) submitForm(this, submitter ?? null);
  });

  // The document's remaining surface.
  method(DocumentProto, 'createEvent', (kind) => {
    const k = String(kind).toLowerCase();
    const Ctor = k.startsWith('mouse') ? MouseEvent : k.startsWith('keyboard') ? KeyboardEvent
      : k.startsWith('custom') ? CustomEvent : k.startsWith('ui') ? UIEvent : Event;
    return new Ctor('');
  });
  method(DocumentProto, 'createAttribute', (name) => new Attr(null, String(name).toLowerCase(), ''));
  method(DocumentProto, 'createAttributeNS', (_ns, name) => new Attr(null, String(name), ''));
  method(DocumentProto, 'createCDATASection', (text) => native.create_text_node(String(text)));
  method(DocumentProto, 'getElementsByName', (name) =>
    document.querySelectorAll(`[name="${String(name).replace(/["\\]/g, '\\$&')}"]`));
  method(DocumentProto, 'elementsFromPoint', () => []);
  method(DocumentProto, 'caretRangeFromPoint', () => null);
  method(DocumentProto, 'caretPositionFromPoint', () => null);
  method(DocumentProto, 'hasStorageAccess', () => Promise.resolve(true));
  method(DocumentProto, 'requestStorageAccess', () => Promise.resolve());
  method(DocumentProto, 'exitFullscreen', () => Promise.resolve());
  method(DocumentProto, 'exitPictureInPicture', () => Promise.resolve());
  method(DocumentProto, 'exitPointerLock', noop);
  method(DocumentProto, 'queryCommandSupported', () => false);
  method(DocumentProto, 'queryCommandEnabled', () => false);
  method(DocumentProto, 'queryCommandState', () => false);
  method(DocumentProto, 'queryCommandValue', () => '');
  method(DocumentProto, 'getAnimations', () => []);
  method(DocumentProto, 'startViewTransition', (update) => {
    const done = Promise.resolve().then(() => (typeof update === 'function' ? update() : undefined));
    return { ready: done, updateCallbackDone: done, finished: done, skipTransition: noop };
  });
  define(document, 'visibilityState', { get: () => 'visible' });
  define(document, 'prerendering', { get: () => false });
  define(document, 'fullscreenElement', { get: () => null });
  define(document, 'fullscreenEnabled', { get: () => false });
  define(document, 'pointerLockElement', { get: () => null });
  define(document, 'pictureInPictureElement', { get: () => null });
  define(document, 'pictureInPictureEnabled', { get: () => false });
  define(document, 'contentType', { get: () => 'text/html' });
  define(document, 'inputEncoding', { get: () => 'UTF-8' });
  define(document, 'designMode', { get: () => 'off', set: noop });
  define(document, 'lastModified', { get: () => '01/01/1970 00:00:00' });
  define(document, 'doctype', {
    get() { for (const c of document.childNodes) if (c.nodeType === 10) return c; return null; },
  });
  define(document, 'embeds', { get: () => document.querySelectorAll('embed') });
  define(document, 'plugins', { get: () => document.querySelectorAll('embed') });
  define(document, 'anchors', { get: () => document.querySelectorAll('a[name]') });
  define(document, 'applets', { get: () => [] });
  define(document, 'styleSheets', {
    get() {
      const list = [...document.querySelectorAll('style, link[rel~="stylesheet"]')].map((el) => el.sheet);
      list.item = (i) => list[i] ?? null;
      return list;
    },
  });
  let adoptedStyleSheets = [];
  define(document, 'adoptedStyleSheets', {
    get: () => adoptedStyleSheets,
    set: (v) => { adoptedStyleSheets = Array.from(v); },
  });
  define(document, 'timeline', { get: () => new globalThis.DocumentTimeline() });
  define(document, 'rootElement', { get: () => null });
  define(document, 'featurePolicy', { get: () => ({ allowsFeature: () => false, features: () => [], allowedFeatures: () => [] }) });

  // A detached <html> element dressed as a document: what `DOMParser` and
  // `createHTMLDocument` hand back, and what a frame's window holds. One
  // arena, so a "new" document is a subtree of the page's own.
  const escapeAttr = (v) => String(v).replace(/["\\]/g, '\\$&');
  globalThis.__mar_document_like = (holder) => {
    if (holder.__mar_is_document) return holder;
    define(holder, '__mar_is_document', { value: true });
    define(holder, 'nodeType', { get: () => 9 });
    define(holder, 'nodeName', { get: () => '#document' });
    define(holder, 'documentElement', { get: () => holder });
    define(holder, 'head', { get: () => holder.querySelector('head') });
    define(holder, 'body', {
      get: () => holder.querySelector('body'),
      set(v) { const old = holder.querySelector('body'); if (old) old.replaceWith(v); else holder.appendChild(v); },
    });
    define(holder, 'title', {
      get: () => holder.querySelector('title')?.textContent ?? '',
      set(v) {
        let t = holder.querySelector('title');
        if (!t) { t = native.create_element('title'); (holder.querySelector('head') || holder).appendChild(t); }
        t.textContent = String(v);
      },
    });
    define(holder, 'defaultView', { get: () => null, configurable: true });
    define(holder, 'readyState', { get: () => 'complete' });
    define(holder, 'implementation', { get: () => document.implementation });
    define(holder, 'doctype', { get: () => null });
    define(holder, 'characterSet', { get: () => 'UTF-8' });
    define(holder, 'contentType', { get: () => 'text/html' });
    define(holder, 'compatMode', { get: () => 'CSS1Compat' });
    define(holder, 'URL', { get: () => 'about:blank' });
    define(holder, 'documentURI', { get: () => 'about:blank' });
    define(holder, 'referrer', { get: () => '' });
    define(holder, 'cookie', { get: () => '', set: noop });
    define(holder, 'currentScript', { get: () => null });
    define(holder, 'activeElement', { get: () => holder.querySelector('body') });
    define(holder, 'hidden', { get: () => true });
    define(holder, 'visibilityState', { get: () => 'hidden' });
    define(holder, 'scripts', { get: () => holder.querySelectorAll('script') });
    define(holder, 'forms', { get: () => holder.querySelectorAll('form') });
    define(holder, 'images', { get: () => holder.querySelectorAll('img') });
    define(holder, 'links', { get: () => holder.querySelectorAll('a[href], area[href]') });
    define(holder, 'styleSheets', { get: () => [] });
    define(holder, 'fonts', { get: () => document.fonts });
    for (const name of ['createElement', 'createElementNS', 'createTextNode', 'createComment',
      'createDocumentFragment', 'createRange', 'createTreeWalker', 'createNodeIterator',
      'createEvent', 'createAttribute', 'createAttributeNS', 'importNode', 'adoptNode',
      'createCDATASection', 'createExpression', 'evaluate']) {
      if (typeof document[name] === 'function') method(holder, name, document[name]);
    }
    method(holder, 'getElementById', (id) => holder.querySelector(`[id="${escapeAttr(id)}"]`));
    method(holder, 'getElementsByName', (n) => holder.querySelectorAll(`[name="${escapeAttr(n)}"]`));
    method(holder, 'open', () => holder);
    method(holder, 'close', noop);
    method(holder, 'write', (...parts) => {
      const body = holder.querySelector('body') || holder;
      body.insertAdjacentHTML('beforeend', parts.join(''));
    });
    method(holder, 'writeln', (...parts) => holder.write(...parts, '\n'));
    method(holder, 'hasFocus', () => false);
    method(holder, 'elementFromPoint', () => null);
    method(holder, 'elementsFromPoint', () => []);
    method(holder, 'execCommand', () => false);
    method(holder, 'getSelection', () => null);
    Object.setPrototypeOf(holder, globalThis.HTMLDocument.prototype);
    return holder;
  };

  // Fonts that are already loaded. Google's home page asks `document.fonts`
  // to load a face before it does anything else, and Vue's transition group
  // waits on `ready`.
  globalThis.FontFace = class FontFace {
    constructor(family, source, descriptors = {}) {
      this.family = String(family); this.status = 'loaded';
      Object.assign(this, { style: 'normal', weight: 'normal', stretch: 'normal', display: 'auto',
        unicodeRange: 'U+0-10FFFF', variant: 'normal', featureSettings: 'normal' }, descriptors);
      this.loaded = Promise.resolve(this);
    }
    load() { return Promise.resolve(this); }
  };
  const fontFaceSet = {
    status: 'loaded', size: 0,
    onloading: null, onloadingdone: null, onloadingerror: null,
    check: () => true,
    load: () => Promise.resolve([]),
    add() { return this; }, delete: () => false, clear: noop, has: () => false,
    forEach: noop, values: () => [][Symbol.iterator](), keys: () => [][Symbol.iterator](),
    entries: () => [][Symbol.iterator](), [Symbol.iterator]: () => [][Symbol.iterator](),
    addEventListener: noop, removeEventListener: noop, dispatchEvent: () => true,
  };
  fontFaceSet.ready = Promise.resolve(fontFaceSet);
  globalThis.FontFaceSet = class FontFaceSet {};
  Object.setPrototypeOf(fontFaceSet, globalThis.FontFaceSet.prototype);
  define(document, 'fonts', { get: () => fontFaceSet });

  // The window's own handler properties, so `window.onload = f` runs `f`
  // when the load event fires and `'onhashchange' in window` says yes.
  const windowHandlers = new Map();
  for (const prop of [
    'onload', 'onerror', 'onunhandledrejection', 'onrejectionhandled', 'onresize', 'onscroll',
    'onscrollend', 'onhashchange', 'onpopstate', 'onmessage', 'onmessageerror', 'onbeforeunload',
    'onunload', 'onpagehide', 'onpageshow', 'onfocus', 'onblur', 'ononline', 'onoffline',
    'onstorage', 'onlanguagechange', 'onorientationchange', 'onbeforeprint', 'onafterprint',
    'onsubmit', 'onchange', 'onclick', 'ondblclick', 'onauxclick', 'oninput', 'onkeydown',
    'onkeyup', 'onkeypress', 'onmousedown', 'onmouseup', 'onmousemove', 'onmouseover',
    'onmouseout', 'onmouseenter', 'onmouseleave', 'onwheel', 'oncontextmenu', 'ontouchstart',
    'ontouchmove', 'ontouchend', 'ontouchcancel', 'onpointerdown', 'onpointerup',
    'onpointermove', 'onpointerover', 'onpointerout', 'onpointerenter', 'onpointerleave',
    'onpointercancel', 'onselect', 'onselectstart', 'onselectionchange', 'onreset',
    'onabort', 'oncanplay', 'onplay', 'onpause', 'onended', 'ontimeupdate', 'onprogress',
    'ondragstart', 'ondrag', 'ondragend', 'ondragenter', 'ondragover', 'ondragleave', 'ondrop',
    'onanimationstart', 'onanimationend', 'onanimationiteration', 'ontransitionend',
    'ontransitionstart', 'onbeforeinput', 'oncopy', 'oncut', 'onpaste', 'ontoggle',
    'onsecuritypolicyviolation', 'ondevicemotion', 'ondeviceorientation', 'ongamepadconnected',
    'ongamepaddisconnected', 'onappinstalled', 'onbeforeinstallprompt', 'onpagereveal',
    'onpageswap', 'onbeforematch', 'oncontextlost', 'oncontextrestored', 'onformdata',
    'onsearch', 'onwebkitanimationend', 'onwebkittransitionend',
  ]) {
    define(globalThis, prop, {
      get: () => windowHandlers.get(prop) ?? null,
      set: (v) => { windowHandlers.set(prop, typeof v === 'function' ? v : null); },
      enumerable: true,
    });
  }
  // And the window is an EventTarget, through `Window.prototype`, for the
  // polyfill that reads `addEventListener` off the prototype chain.
  // Named access: `window.sidebar` is the element with id "sidebar", and
  // `window.frameName` is that frame's window. They sit on an object below
  // `Window.prototype`, as the spec puts them, so a page's own `var sidebar`
  // shadows the element rather than colliding with it.
  const namedProperties = Object.create(globalThis.EventTarget.prototype);
  const nameElement = (name, resolve) => {
    if (!name || name in globalThis || Object.prototype.hasOwnProperty.call(namedProperties, name)) return;
    define(namedProperties, name, {
      get: resolve,
      set(v) { define(this, name, { value: v, writable: true, enumerable: true }); },
      enumerable: false,
    });
  };
  const nameTheDocument = () => {
    for (const el of document.querySelectorAll('[id]')) {
      const id = el.id;
      nameElement(id, () => document.getElementById(id) ?? undefined);
    }
    for (const el of document.querySelectorAll('iframe[name], form[name], img[name], embed[name], object[name]')) {
      const name = el.getAttribute('name');
      if (el.tagName === 'IFRAME') {
        nameElement(name, () => {
          const frame = document.querySelector(`iframe[name="${name.replace(/"/g, '\\"')}"]`);
          return frame ? frame.contentWindow : undefined;
        });
      } else {
        nameElement(name, () => document.querySelector(`[name="${name.replace(/"/g, '\\"')}"]`) ?? undefined);
      }
    }
  };
  globalThis.__mar_name_the_document = nameTheDocument;
  const Window = function Window() { throw new TypeError('Illegal constructor: Window'); };
  Window.prototype = Object.create(namedProperties, {
    constructor: { value: Window, writable: true, configurable: true },
  });
  define(globalThis, 'Window', { value: Window, writable: true });
  try { Object.setPrototypeOf(globalThis, Window.prototype); } catch (e) { /* keep Object.prototype */ }
  nameTheDocument();

  // Odds and ends a page reads off the platform.
  Object.assign(navigator, {
    connection: { effectiveType: '4g', downlink: 10, rtt: 50, saveData: false, type: 'wifi',
      onchange: null, addEventListener: noop, removeEventListener: noop },
    locks: {
      request: (name, options, callback) => {
        const run = typeof options === 'function' ? options : callback;
        return Promise.resolve().then(() => (typeof run === 'function' ? run({ name: String(name), mode: 'exclusive' }) : undefined));
      },
      query: () => Promise.resolve({ held: [], pending: [] }),
    },
    mediaDevices: {
      enumerateDevices: () => Promise.resolve([]),
      getUserMedia: () => Promise.reject(new globalThis.DOMException('no devices', 'NotFoundError')),
      getDisplayMedia: () => Promise.reject(new globalThis.DOMException('no devices', 'NotFoundError')),
      getSupportedConstraints: () => ({}),
      ondevicechange: null, addEventListener: noop, removeEventListener: noop,
    },
    geolocation: {
      getCurrentPosition: (_ok, fail) => {
        if (typeof fail === 'function') setTimeout(() => fail({ code: 1, message: 'User denied Geolocation', PERMISSION_DENIED: 1, POSITION_UNAVAILABLE: 2, TIMEOUT: 3 }), 0);
      },
      watchPosition: (_ok, fail) => {
        if (typeof fail === 'function') setTimeout(() => fail({ code: 1, message: 'User denied Geolocation', PERMISSION_DENIED: 1, POSITION_UNAVAILABLE: 2, TIMEOUT: 3 }), 0);
        return 1;
      },
      clearWatch: noop,
    },
    storage: {
      estimate: () => Promise.resolve({ quota: 2 ** 33, usage: 0, usageDetails: {} }),
      persist: () => Promise.resolve(false), persisted: () => Promise.resolve(false),
      getDirectory: () => Promise.reject(new globalThis.DOMException('no origin private file system', 'NotSupportedError')),
    },
    credentials: {
      get: () => Promise.resolve(null), store: () => Promise.resolve(), create: () => Promise.resolve(null),
      preventSilentAccess: () => Promise.resolve(),
    },
    share: () => Promise.reject(new globalThis.DOMException('no share target', 'AbortError')),
    canShare: () => false,
    vibrate: () => false,
    getBattery: () => Promise.resolve({ charging: true, level: 1, chargingTime: 0, dischargingTime: Infinity,
      onchargingchange: null, onlevelchange: null, addEventListener: noop, removeEventListener: noop }),
    getGamepads: () => [],
    scheduling: { isInputPending: () => false },
    mediaSession: { metadata: null, playbackState: 'none', setActionHandler: noop, setPositionState: noop },
    wakeLock: { request: () => Promise.reject(new globalThis.DOMException('no wake lock', 'NotAllowedError')) },
    mediaCapabilities: { decodingInfo: () => Promise.resolve({ supported: false, smooth: false, powerEfficient: false }),
      encodingInfo: () => Promise.resolve({ supported: false, smooth: false, powerEfficient: false }) },
    userActivation: { hasBeenActive: false, isActive: false },
    appName: 'Netscape', appCodeName: 'Mozilla', product: 'Gecko', productSub: '20030107', vendorSub: '',
    pdfViewerEnabled: false, globalPrivacyControl: false,
    requestMediaKeySystemAccess: () => Promise.reject(new globalThis.DOMException('no keys', 'NotSupportedError')),
    registerProtocolHandler: noop, unregisterProtocolHandler: noop,
    setAppBadge: () => Promise.resolve(), clearAppBadge: () => Promise.resolve(),
  });
  globalThis.visualViewport = {
    width: innerWidth, height: innerHeight, offsetLeft: 0, offsetTop: 0, pageLeft: 0, pageTop: 0,
    scale: 1, onresize: null, onscroll: null, onscrollend: null,
    addEventListener: noop, removeEventListener: noop, dispatchEvent: () => true,
  };
  globalThis.scheduler = {
    postTask: (callback, options = {}) => new Promise((resolve, reject) => {
      setTimeout(() => { try { resolve(callback()); } catch (e) { reject(e); } }, options.delay ?? 0);
    }),
    yield: () => Promise.resolve(),
  };
  globalThis.PerformanceObserver.supportedEntryTypes = [
    'element', 'event', 'first-input', 'largest-contentful-paint', 'layout-shift',
    'long-animation-frame', 'longtask', 'mark', 'measure', 'navigation', 'paint',
    'resource', 'visibility-state',
  ];
  const timeOrigin = Date.now();
  performance.timeOrigin = timeOrigin;
  performance.timing = {
    navigationStart: timeOrigin, unloadEventStart: 0, unloadEventEnd: 0, redirectStart: 0,
    redirectEnd: 0, fetchStart: timeOrigin + 1, domainLookupStart: timeOrigin + 1,
    domainLookupEnd: timeOrigin + 2, connectStart: timeOrigin + 2, secureConnectionStart: timeOrigin + 3,
    connectEnd: timeOrigin + 5, requestStart: timeOrigin + 5, responseStart: timeOrigin + 20,
    responseEnd: timeOrigin + 30, domLoading: timeOrigin + 31, domInteractive: timeOrigin + 50,
    domContentLoadedEventStart: timeOrigin + 50, domContentLoadedEventEnd: timeOrigin + 51,
    domComplete: timeOrigin + 60, loadEventStart: timeOrigin + 60, loadEventEnd: timeOrigin + 61,
    toJSON() { return { ...this }; },
  };
  performance.navigation = { type: 0, redirectCount: 0, TYPE_NAVIGATE: 0, TYPE_RELOAD: 1, TYPE_BACK_FORWARD: 2, TYPE_RESERVED: 255, toJSON() { return { type: 0, redirectCount: 0 }; } };
  performance.memory = { usedJSHeapSize: 10_000_000, totalJSHeapSize: 20_000_000, jsHeapSizeLimit: 2_000_000_000 };
  performance.eventCounts = new Map();
  performance.toJSON = () => ({ timeOrigin });
  performance.setResourceTimingBufferSize = noop;
  performance.clearResourceTimings = noop;
  performance.addEventListener = noop;
  performance.removeEventListener = noop;
  const navigationEntry = {
    name: location.href, entryType: 'navigation', startTime: 0, duration: 61, initiatorType: 'navigation',
    nextHopProtocol: 'h2', workerStart: 0, redirectStart: 0, redirectEnd: 0, fetchStart: 1,
    domainLookupStart: 1, domainLookupEnd: 2, connectStart: 2, secureConnectionStart: 3, connectEnd: 5,
    requestStart: 5, responseStart: 20, responseEnd: 30, transferSize: 0, encodedBodySize: 0,
    decodedBodySize: 0, unloadEventStart: 0, unloadEventEnd: 0, domInteractive: 50,
    domContentLoadedEventStart: 50, domContentLoadedEventEnd: 51, domComplete: 60, loadEventStart: 60,
    loadEventEnd: 61, type: 'navigate', redirectCount: 0, activationStart: 0, serverTiming: [],
    toJSON() { return { ...this }; },
  };
  performance.getEntriesByType = (type) => (type === 'navigation' ? [navigationEntry] : []);
  performance.getEntries = () => [navigationEntry];
  performance.getEntriesByName = (name) => (name === location.href ? [navigationEntry] : []);
  globalThis.PerformanceNavigationTiming = class PerformanceNavigationTiming {};
  globalThis.PerformanceResourceTiming = class PerformanceResourceTiming {};
  globalThis.PerformancePaintTiming = class PerformancePaintTiming {};
  globalThis.Notification = class Notification {
    constructor(title, options = {}) { this.title = String(title); Object.assign(this, options); }
    static get permission() { return 'default'; }
    static requestPermission(callback) {
      if (typeof callback === 'function') setTimeout(() => callback('default'), 0);
      return Promise.resolve('default');
    }
    static get maxActions() { return 0; }
    close() {} addEventListener() {} removeEventListener() {} dispatchEvent() { return true; }
  };
  // An IndexedDB that declines. A bare `indexedDB` is a ReferenceError
  // without this, and a request that never answers leaves a page waiting; a
  // request that fails is the state a browser with storage disabled reports,
  // and every library has a path for it.
  const idbRequest = () => {
    const request = {
      readyState: 'pending', result: undefined, error: null, source: null, transaction: null,
      onsuccess: null, onerror: null, onupgradeneeded: null, onblocked: null,
      _listeners: [],
      addEventListener(type, fn) { this._listeners.push([type, fn]); },
      removeEventListener(type, fn) { this._listeners = this._listeners.filter(([t, f]) => t !== type || f !== fn); },
      dispatchEvent() { return true; },
    };
    setTimeout(() => {
      request.readyState = 'done';
      request.error = new globalThis.DOMException('IndexedDB is not available here', 'UnknownError');
      const event = new Event('error', { bubbles: true, cancelable: true });
      event.target = request;
      for (const [type, fn] of request._listeners) if (type === 'error') { try { (fn.handleEvent || fn).call(request, event); } catch (e) { native.record_error('indexedDB', describeError(e)); } }
      if (typeof request.onerror === 'function') { try { request.onerror(event); } catch (e) { native.record_error('indexedDB', describeError(e)); } }
    }, 0);
    return request;
  };
  globalThis.indexedDB = {
    open: idbRequest, deleteDatabase: idbRequest,
    databases: () => Promise.resolve([]),
    cmp: (a, b) => (a < b ? -1 : a > b ? 1 : 0),
  };
  globalThis.IDBFactory = class IDBFactory {};
  globalThis.IDBRequest = class IDBRequest {};
  globalThis.IDBOpenDBRequest = class IDBOpenDBRequest {};
  globalThis.IDBDatabase = class IDBDatabase {};
  globalThis.IDBTransaction = class IDBTransaction {};
  globalThis.IDBObjectStore = class IDBObjectStore {};
  globalThis.IDBIndex = class IDBIndex {};
  globalThis.IDBCursor = class IDBCursor {};
  globalThis.IDBKeyRange = class IDBKeyRange {
    static only() { return new IDBKeyRange(); } static bound() { return new IDBKeyRange(); }
    static lowerBound() { return new IDBKeyRange(); } static upperBound() { return new IDBKeyRange(); }
  };

  globalThis.IntersectionObserverEntry = class IntersectionObserverEntry {};
  globalThis.ResizeObserverEntry = class ResizeObserverEntry {};
  globalThis.ResizeObserverSize = class ResizeObserverSize {};
  globalThis.MediaQueryListEvent = class MediaQueryListEvent extends Event {};
  globalThis.ScreenOrientation = class ScreenOrientation {};
  Object.assign(screen.orientation, { lock: () => Promise.reject(new globalThis.DOMException('no lock', 'NotSupportedError')), unlock: noop, addEventListener: noop, removeEventListener: noop, onchange: null });
  globalThis.matchMedia = ((real) => (query) => {
    const list = real(query);
    return Object.setPrototypeOf(list, globalThis.MediaQueryList.prototype);
  })(globalThis.matchMedia);
  globalThis.onorientationchange = null;
  globalThis.orientation = 0;
  globalThis.status = '';
  globalThis.defaultStatus = '';
  globalThis.toolbar = { visible: false };
  globalThis.menubar = { visible: false };
  globalThis.locationbar = { visible: false };
  globalThis.personalbar = { visible: false };
  globalThis.scrollbars = { visible: false };
  globalThis.statusbar = { visible: false };
  globalThis.screenX = 0; globalThis.screenY = 0; globalThis.screenLeft = 0; globalThis.screenTop = 0;
  globalThis.getScreenDetails = () => Promise.reject(new globalThis.DOMException('no screens', 'NotAllowedError'));
  globalThis.queryLocalFonts = () => Promise.resolve([]);
  globalThis.showOpenFilePicker = () => Promise.reject(new globalThis.DOMException('no picker', 'AbortError'));
  globalThis.showSaveFilePicker = globalThis.showOpenFilePicker;
  globalThis.showDirectoryPicker = globalThis.showOpenFilePicker;
  globalThis.createImageBitmap = () => Promise.resolve({ width: 0, height: 0, close: noop });
  globalThis.ImageBitmap = class ImageBitmap {};
  globalThis.captureEvents = noop;
  globalThis.releaseEvents = noop;
  globalThis.find = () => false;
  globalThis.getDigitalGoodsService = undefined;

  // -- error reporting -----------------------------------------------------

  globalThis.onerror = null;
  globalThis.onunhandledrejection = null;
  globalThis.reportError = (e) => native.record_error('reportError', describeError(e));

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
    // Elements the scripts added while parsing get their names on the window
    // before the listeners that might use them run.
    try { globalThis.__mar_name_the_document(); } catch (e) { /* nothing to name */ }
    try {
      dispatchEvent.call(document, new Event('DOMContentLoaded', { bubbles: true }));
    } catch (e) {
      native.record_error('DOMContentLoaded', describeError(e));
    }
    try {
      dispatchEvent.call(globalThis, new Event('load'));
    } catch (e) {
      native.record_error('load', describeError(e));
    }
    try {
      dispatchEvent.call(document, new Event('readystatechange'));
    } catch (e) {
      native.record_error('readystatechange', describeError(e));
    }
  };

  // -- looking built in ----------------------------------------------------

  // The bridge the CDP layer calls sits on `globalThis` in plain sight of
  // `Object.keys`; it says plainly what is running the page, so it is hidden
  // from enumeration. Everything else here already looks built in: the
  // prelude runs from bytecode with its source stripped, and QuickJS prints
  // such a function as `[native code]`, exactly as a browser prints its own.
  for (const name of Object.getOwnPropertyNames(globalThis)) {
    if (name.startsWith('__mar')) {
      define(globalThis, name, { enumerable: false, writable: true, value: globalThis[name] });
    }
  }
  // QuickJS prints a source-less function over three lines; Chrome prints
  // its own on one. The one-line form is what a page compares against.
  const realToString = Function.prototype.toString;
  const masked = function toString() {
    const source = realToString.call(this);
    return source.includes('[native code]')
      ? `function ${typeof this === 'function' && this.name ? this.name : ''}() { [native code] }`
      : source;
  };
  method(Function.prototype, 'toString', masked);

  // `globalThis.fetch = (...) => ...` leaves the function anonymous, and a
  // page reads `fetch.name`. One level, no recursion: the names below are
  // where a page looks, and `method()` has already named the rest.
  for (const holder of [globalThis, navigator, location, history, performance, screen, console]) {
    if (!holder) continue;
    for (const name of Object.getOwnPropertyNames(holder)) {
      const desc = Object.getOwnPropertyDescriptor(holder, name);
      if (desc && typeof desc.value === 'function' && !desc.value.name) {
        try { define(desc.value, 'name', { value: name }); } catch (e) { /* frozen */ }
      }
    }
  }

  // A browser keeps each member on the interface that defines it, and code
  // that patches the DOM reads the descriptor off that prototype and writes
  // its own there: ShadyDOM, which YouTube forces on even in Chrome, takes
  // `innerHTML` from Element.prototype and `firstChild` from Node.prototype.
  // Everything above was put on Node.prototype for brevity. This moves each
  // member where a browser has it, so a text node no longer answers to
  // `innerHTML` and a descriptor read off Element.prototype finds one. What
  // nothing below names stays on Node; a tag's own members (an anchor's
  // `href`, an input's `value`) go to HTMLElement rather than to one
  // prototype per tag, which is where feature detection looks for them.
  {
    const N = NodeProto;
    const words = (s) => s.split(/\s+/).filter(Boolean);
    const place = (names, ...targets) => {
      for (const name of names) {
        const desc = Object.getOwnPropertyDescriptor(N, name);
        if (!desc) continue;
        for (const target of targets) Object.defineProperty(target.prototype, name, desc);
        delete N[name];
      }
    };
    place(words(`append prepend replaceChildren querySelector querySelectorAll getElementsByTagName
      getElementsByClassName children firstElementChild lastElementChild childElementCount`),
      Element, Document, DocumentFragment);
    place(words('remove before after replaceWith nextElementSibling previousElementSibling'), Element, CharacterData);
    // A shadow root reads and writes `innerHTML` as an element does.
    place(words('innerHTML'), Element, ShadowRoot);
    const aria = Object.getOwnPropertyNames(N).filter((n) => n.startsWith('aria'));
    place(words(`getAttribute setAttribute hasAttribute removeAttribute toggleAttribute getAttributeNames matches
      closest className outerHTML tagName id localName namespaceURI classList slot part attributes attachShadow
      shadowRoot assignedSlot insertAdjacentHTML insertAdjacentElement insertAdjacentText getBoundingClientRect
      getClientRects scrollIntoView scrollIntoViewIfNeeded scrollTo scroll scrollBy clientWidth clientHeight
      scrollWidth scrollHeight scrollTop scrollLeft checkVisibility getAnimations animate attributeStyleMap
      elementTiming role getAttributeNode getAttributeNodeNS setAttributeNode setAttributeNodeNS removeAttributeNode
      hasAttributes getAttributeNS setAttributeNS hasAttributeNS removeAttributeNS requestFullscreen`).concat(aria),
      Element);
    const handlers = Object.getOwnPropertyNames(N).filter((n) => /^on[a-z]/.test(n));
    place(handlers, HTMLElement, SVGElement, Document);
    place(words('hidden dir'), HTMLElement);
    // The document's `hidden` is its visibility, and its `dir` is the root
    // element's; neither is an attribute of the document itself.
    define(Document.prototype, 'hidden', { get() { return this.visibilityState === 'hidden'; } });
    define(Document.prototype, 'dir', {
      get() { return this.documentElement ? this.documentElement.dir : ''; },
      set(v) { if (this.documentElement) this.documentElement.dir = v; },
    });
    place(words('style dataset nonce autofocus tabIndex focus blur'), HTMLElement, SVGElement);
    const node = new Set(words(`constructor contains hasChildNodes appendChild insertBefore removeChild replaceChild
      cloneNode parentNode lastChild parentElement childNodes nodeValue firstChild nodeName textContent nodeType
      nextSibling ownerDocument previousSibling marNodeId normalize isSameNode isEqualNode getRootNode
      compareDocumentPosition lookupNamespaceURI lookupPrefix isDefaultNamespace isConnected addEventListener
      removeEventListener dispatchEvent data templateContent`));
    place(Object.getOwnPropertyNames(N).filter((n) => !node.has(n) && !/^[A-Z_]+$/.test(n)), HTMLElement);

    // What a tag defines goes on to the tag's own interface, where a script
    // that patches `HTMLScriptElement.prototype.src` — every consent manager
    // that gates third-party scripts — reads the descriptor. What no tag
    // below claims stays on HTMLElement.
    const H = HTMLElement.prototype;
    const settle = (names, ...targets) => {
      for (const name of words(names)) {
        const desc = Object.getOwnPropertyDescriptor(H, name);
        if (!desc) continue;
        for (const target of targets) Object.defineProperty(target.prototype, name, desc);
        delete H[name];
      }
    };
    settle('src', HTMLScriptElement, HTMLImageElement, HTMLIFrameElement, HTMLMediaElement, HTMLSourceElement,
      HTMLEmbedElement, HTMLTrackElement, HTMLInputElement);
    settle('srcset', HTMLImageElement, HTMLSourceElement);
    settle('sizes', HTMLImageElement, HTMLSourceElement, HTMLLinkElement);
    settle('currentSrc', HTMLImageElement, HTMLMediaElement);
    settle('crossOrigin', HTMLImageElement, HTMLMediaElement, HTMLScriptElement, HTMLLinkElement);
    settle('referrerPolicy', HTMLAnchorElement, HTMLAreaElement, HTMLImageElement, HTMLIFrameElement, HTMLLinkElement,
      HTMLScriptElement);
    settle('integrity', HTMLScriptElement, HTMLLinkElement);
    settle('loading', HTMLImageElement, HTMLIFrameElement);
    settle('href', HTMLAnchorElement, HTMLLinkElement, HTMLAreaElement, HTMLBaseElement);
    settle('target', HTMLAnchorElement, HTMLAreaElement, HTMLBaseElement, HTMLFormElement);
    settle('download rel hreflang', HTMLAnchorElement, HTMLAreaElement, HTMLLinkElement);
    settle('protocol host hostname port pathname search hash origin username password', HTMLAnchorElement, HTMLAreaElement);
    settle('coords shape', HTMLAreaElement);
    settle('text', HTMLScriptElement, HTMLAnchorElement, HTMLTitleElement, HTMLOptionElement, HTMLBodyElement);
    settle('content', HTMLMetaElement, HTMLTemplateElement);
    settle('charset', HTMLScriptElement, HTMLMetaElement);
    settle('httpEquiv scheme', HTMLMetaElement);
    settle('media', HTMLLinkElement, HTMLStyleElement, HTMLSourceElement);
    settle('as', HTMLLinkElement);
    settle('sheet', HTMLLinkElement, HTMLStyleElement);
    settle('value', HTMLInputElement, HTMLTextAreaElement, HTMLSelectElement, HTMLOptionElement, HTMLButtonElement,
      HTMLProgressElement, HTMLMeterElement, HTMLDataElement, HTMLLIElement, HTMLOutputElement);
    settle('defaultValue', HTMLInputElement, HTMLTextAreaElement, HTMLOutputElement);
    settle('checked defaultChecked indeterminate files valueAsNumber stepUp stepDown pattern accept', HTMLInputElement);
    settle('select setSelectionRange setRangeText selectionStart selectionEnd selectionDirection placeholder maxLength minLength readOnly',
      HTMLInputElement, HTMLTextAreaElement);
    settle('alt', HTMLImageElement, HTMLInputElement, HTMLAreaElement);
    settle('autocomplete', HTMLInputElement, HTMLFormElement, HTMLSelectElement, HTMLTextAreaElement);
    settle('min max', HTMLInputElement, HTMLMeterElement, HTMLProgressElement);
    settle('step', HTMLInputElement);
    settle('size', HTMLInputElement, HTMLSelectElement);
    settle('multiple', HTMLInputElement, HTMLSelectElement);
    settle('required', HTMLInputElement, HTMLSelectElement, HTMLTextAreaElement);
    settle('disabled', HTMLInputElement, HTMLButtonElement, HTMLSelectElement, HTMLTextAreaElement, HTMLOptionElement,
      HTMLOptGroupElement, HTMLFieldSetElement, HTMLLinkElement, HTMLStyleElement);
    settle('selected', HTMLOptionElement);
    settle('selectedIndex selectedOptions', HTMLSelectElement);
    settle('options', HTMLSelectElement, HTMLDataListElement);
    settle('form', HTMLInputElement, HTMLButtonElement, HTMLSelectElement, HTMLTextAreaElement, HTMLOptionElement,
      HTMLLabelElement, HTMLFieldSetElement, HTMLOutputElement, HTMLObjectElement, HTMLLegendElement);
    settle('labels', HTMLInputElement, HTMLButtonElement, HTMLSelectElement, HTMLTextAreaElement, HTMLMeterElement,
      HTMLOutputElement, HTMLProgressElement);
    settle('validity validationMessage willValidate checkValidity reportValidity setCustomValidity', HTMLInputElement,
      HTMLButtonElement, HTMLSelectElement, HTMLTextAreaElement, HTMLFieldSetElement, HTMLOutputElement, HTMLObjectElement);
    settle('elements', HTMLFormElement, HTMLFieldSetElement);
    settle('submit requestSubmit reset action method enctype', HTMLFormElement);
    settle('rows cols wrap', HTMLTextAreaElement);
    settle('htmlFor', HTMLLabelElement, HTMLOutputElement);
    settle('width height', HTMLImageElement, HTMLCanvasElement, HTMLVideoElement, HTMLIFrameElement, HTMLEmbedElement,
      HTMLObjectElement, HTMLInputElement, HTMLSourceElement);
    settle('naturalWidth naturalHeight complete decode', HTMLImageElement);
    settle('useMap', HTMLImageElement, HTMLObjectElement);
    settle('getContext toDataURL toBlob', HTMLCanvasElement);
    settle('captureStream', HTMLCanvasElement, HTMLMediaElement);
    settle(`canPlayType play pause load paused ended muted volume currentTime duration playbackRate networkState seeking
      autoplay loop controls playsInline defaultMuted buffered played seekable textTracks preload`, HTMLMediaElement);
    settle('poster videoWidth videoHeight requestPictureInPicture', HTMLVideoElement);
    settle('kind srclang', HTMLTrackElement);
    settle('label', HTMLTrackElement, HTMLOptionElement, HTMLOptGroupElement);
    settle('contentWindow contentDocument', HTMLIFrameElement, HTMLObjectElement);
    settle('getSVGDocument', HTMLIFrameElement, HTMLObjectElement, HTMLEmbedElement);
    settle('open', HTMLDialogElement, HTMLDetailsElement);
    settle('showModal show close', HTMLDialogElement);
    settle('dateTime', HTMLTimeElement, HTMLModElement);
    settle('cite', HTMLQuoteElement, HTMLModElement);
    settle('headers abbr scope axis', HTMLTableCellElement);
    settle('popoverTarget', HTMLButtonElement, HTMLInputElement);
  }
})();
