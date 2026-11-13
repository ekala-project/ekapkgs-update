// Starts the Elm dashboard (elm.js) and feeds it live-update notifications.
//
// The server pushes {"type":"live","v":"<fingerprint>"} on /ws/live when
// the database changes. A fingerprint different from the last one seen is
// sent to the `liveUpdate` port, and the app re-fetches the page's data.
// Connection state goes to the `liveStatus` port for the header indicator.
// The last fingerprint survives reconnects, so a reconnect only refreshes
// the page if something changed while the socket was down.
(function () {
  "use strict";

  var MIN_DELAY = 1000;
  var MAX_DELAY = 30000;

  var app = window.Elm.Main.init({ flags: { now: Date.now() } });

  var lastVersion = null;
  var delay = MIN_DELAY;
  var socket = null;
  var retryTimer = null;

  function send(port, value) {
    if (app.ports && app.ports[port]) app.ports[port].send(value);
  }

  function setState(state, label) {
    send("liveStatus", { state: state, label: label });
  }

  function scheduleReconnect() {
    if (retryTimer !== null) return;
    var seconds = Math.round(delay / 1000);
    setState("offline", "offline, retrying in " + seconds + "s");
    retryTimer = window.setTimeout(function () {
      retryTimer = null;
      connect();
    }, delay);
    delay = Math.min(delay * 2, MAX_DELAY);
  }

  function onMessage(event) {
    var msg;
    try {
      msg = JSON.parse(event.data);
    } catch (_) {
      return;
    }
    if (!msg || msg.type !== "live" || typeof msg.v !== "string") return;
    if (lastVersion !== null && msg.v !== lastVersion) send("liveUpdate", msg.v);
    lastVersion = msg.v;
  }

  function connect() {
    if (!("WebSocket" in window)) {
      setState("offline", "unsupported; data refreshes every minute");
      return;
    }
    setState("connecting", "connecting");
    var scheme = window.location.protocol === "https:" ? "wss:" : "ws:";
    try {
      socket = new WebSocket(scheme + "//" + window.location.host + "/ws/live");
    } catch (_) {
      scheduleReconnect();
      return;
    }
    socket.addEventListener("open", function () {
      delay = MIN_DELAY;
      setState("live", "live");
    });
    socket.addEventListener("message", onMessage);
    socket.addEventListener("close", function () {
      socket = null;
      scheduleReconnect();
    });
  }

  // Reconnect immediately when the tab becomes visible again after an outage.
  document.addEventListener("visibilitychange", function () {
    if (document.visibilityState === "visible" && socket === null && retryTimer !== null) {
      window.clearTimeout(retryTimer);
      retryTimer = null;
      delay = MIN_DELAY;
      connect();
    }
  });

  connect();
})();
