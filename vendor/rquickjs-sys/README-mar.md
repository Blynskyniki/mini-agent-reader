# rquickjs-sys, vendored

This is `rquickjs-sys` 0.12.2 from crates.io with the test suites, examples
and documentation of the bundled QuickJS-ng removed, and one change to
`quickjs/quickjs.c`, marked with `mar:` comments:

- `for (f() in o)` and `for (f() of o)` parse. QuickJS rejected a call
  expression as the loop target with "invalid for in/of left hand-side";
  V8 accepts it, runs the call, and throws a ReferenceError only when it has
  something to assign, so `for (f() in [])` completes. Bot-detection scripts
  (servicepipe.tech, on tass.ru and lamoda.ru among others) wrap exactly that
  in `try/catch` to tell a browser from another engine, and with the parse
  error the whole script failed.

Everything else is upstream. To update: copy the new crate over this
directory, drop the same files, and re-apply the change.
