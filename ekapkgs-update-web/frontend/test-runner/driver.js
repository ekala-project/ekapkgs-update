// Runs the compiled test runner under QuickJS:
//
//   qjs --std driver.js <runner.js> <seed>
//
// `std` and `os` are QuickJS globals under --std.

// QuickJS has no console.warn (called by elm's DEV-mode banner) and keeps
// its timers in `os`; the elm scheduler needs setTimeout.
console.warn = function () {};
globalThis.setTimeout = os.setTimeout;
globalThis.clearTimeout = os.clearTimeout;

std.loadScript(scriptArgs[1]);

var seed = parseInt(scriptArgs[2], 10);
var done = false;
var app = globalThis.Elm.TestMain.init({ flags: null });

app.ports.report.subscribe(function (result) {
  done = true;
  std.out.puts(result.text + "\n");
  std.out.flush();
  std.exit(result.failed === 0 ? 0 : 1);
});

app.ports.start.send(seed);

// The elm scheduler runs on timers, so the report arrives asynchronously;
// fail rather than exit 0 if it never does.
os.setTimeout(function () {
  if (!done) {
    std.err.puts("The test runner did not report a result.\n");
    std.exit(2);
  }
}, 600000);
