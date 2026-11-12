// Live updates for the ekapkgs-update dashboard.
//
// The server pushes {"type":"live","v":"<fingerprint>"} on /ws/live when
// the database changes. When the fingerprint differs from the last one seen,
// a `live-update` event is dispatched on <body>; panels that should refresh
// declare `hx-trigger="live-update from:body"` and re-fetch their fragment.
// The last fingerprint survives reconnects, so a reconnect only refreshes
// panels if something changed while the socket was down.
(function () {
  "use strict";

  var MIN_DELAY = 1000;
  var MAX_DELAY = 30000;

  var lastVersion = null;
  var delay = MIN_DELAY;
  var socket = null;
  var retryTimer = null;

  function setState(state, label) {
    var el = document.getElementById("live-status");
    if (!el) return;
    el.setAttribute("data-state", state);
    el.setAttribute("title", "Live updates: " + label);
    var text = el.querySelector(".live-label");
    if (text) text.textContent = label;
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
    if (lastVersion !== null && msg.v !== lastVersion) {
      document.body.dispatchEvent(new CustomEvent("live-update", { detail: { version: msg.v } }));
    }
    lastVersion = msg.v;
  }

  function connect() {
    if (!("WebSocket" in window)) {
      setState("offline", "unsupported; panels refresh every minute");
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
    socket.addEventListener("error", function () {
      // A close event always follows; reconnect is handled there.
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

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", connect);
  } else {
    connect();
  }
})();
