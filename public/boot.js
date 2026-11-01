// Runs before the module bundle, so the very first layout is already at the
// remembered size even if the reveal below beats the real application of zoom.
//
// A file rather than a script inside index.html: the recording service serves
// this interface to browsers under a policy that runs no inline script (see
// src-tauri/src/recording/web.rs), and the app's window loads it the same way.
//
// The key and the bounds are duplicated from src/logic/zoom.ts because nothing
// importable exists this early. src/__checks__/zoom.test.mjs asserts the two
// agree, so a change in one cannot drift from the other.
(function () {
  var root = document.documentElement;
  root.classList.add("zoom-pending");
  var zoom = 1;
  try {
    var stored = parseFloat(localStorage.getItem("zapper.zoom"));
    if (isFinite(stored) && stored > 0) {
      zoom = Math.min(2, Math.max(0.7, stored));
    }
  } catch (e) {
    // Storage unavailable. 100% is the right answer and the app still opens,
    // which is the only thing that matters here.
  }
  root.style.setProperty("--zoom", String(zoom));
  root.style.zoom = String(zoom);
  // If the bundle fails to load or throws before it applies zoom, the
  // interface must still appear. CSS zoom above is already correct, so
  // revealing on a timer costs nothing but removes the only way this could
  // hide the app permanently.
  setTimeout(function () {
    root.classList.remove("zoom-pending");
  }, 600);
})();
