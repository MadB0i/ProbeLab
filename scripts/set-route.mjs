/**
 * Points a running ProbeLab window at a route and waits for it to settle.
 *
 * # Why this exists
 *
 * Screenshots of the real application have to come from the real executable -- a browser render with
 * a stubbed IPC boundary cannot show the native chrome, the WebView2 font rasterisation, or what the
 * actual backend returns. But the app routes on `location.hash`, and an already-running process has
 * no way to be told to change it: there is no URL argument, no IPC command for it, and no handle
 * into the WebView from outside the process.
 *
 * The WebView2 runtime accepts `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`, so the window can be started
 * with `--remote-debugging-port`. That opens the DevTools protocol on loopback only, which is what
 * this script speaks to set the hash. It drives nothing else: the page still renders through its own
 * React state, its own polling, and its own backend calls.
 *
 * Exit code 1 also reports the browser console errors seen while the route was on screen, because a
 * screenshot that looks right and throws on every render is not a finished screen.
 *
 * Usage: node scripts/set-route.mjs <route> [port] [settle-ms]
 */
const route = process.argv[2];
const port = process.argv[3] ?? "9333";
const settleMs = Number(process.argv[4] ?? 1500);

if (!route) {
  console.error("usage: node scripts/set-route.mjs <route> [port] [settle-ms]");
  process.exit(2);
}

const target = await fetch(`http://127.0.0.1:${port}/json/list`)
  .then((response) => response.json())
  .catch((error) => {
    console.error(`no DevTools endpoint on 127.0.0.1:${port}: ${error.message}`);
    process.exit(2);
  })
  .then((targets) => targets.find((entry) => entry.type === "page" && entry.webSocketDebuggerUrl));

if (!target) {
  console.error("no page target exposed by the running window");
  process.exit(2);
}

const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve, { once: true });
  socket.addEventListener("error", reject, { once: true });
});

let nextId = 0;
const pending = new Map();
const consoleErrors = [];

socket.addEventListener("message", (event) => {
  const message = JSON.parse(event.data);
  if (message.id && pending.has(message.id)) {
    const { resolve, reject } = pending.get(message.id);
    pending.delete(message.id);
    message.error ? reject(new Error(message.error.message)) : resolve(message.result);
    return;
  }
  // Log.entryAdded carries the browser's own view (uncaught errors, failed requests);
  // Runtime.consoleAPICalled catches console.error from the application.
  if (message.method === "Log.entryAdded" && message.params.entry.level === "error") {
    consoleErrors.push(message.params.entry.text);
  }
  if (message.method === "Runtime.consoleAPICalled" && message.params.type === "error") {
    consoleErrors.push(
      message.params.args.map((argument) => argument.value ?? argument.description ?? "").join(" "),
    );
  }
  if (message.method === "Runtime.exceptionThrown") {
    const detail = message.params.exceptionDetails;
    consoleErrors.push(detail.text + (detail.exception?.description ? ` ${detail.exception.description}` : ""));
  }
});

function send(method, params = {}) {
  const id = ++nextId;
  socket.send(JSON.stringify({ id, method, params }));
  return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
}

await send("Page.enable");
await send("Runtime.enable");
await send("Log.enable");

await send("Runtime.evaluate", {
  expression: `window.location.hash = ${JSON.stringify(`#/${route}`)}`,
});

await new Promise((resolve) => setTimeout(resolve, settleMs));

const { result } = await send("Runtime.evaluate", {
  expression: `JSON.stringify({
    hash: location.hash,
    title: document.querySelector("h1")?.textContent ?? "",
    ready: document.readyState,
    width: document.documentElement.clientWidth,
    height: document.documentElement.clientHeight,
  })`,
  returnByValue: true,
});

const state = JSON.parse(result.value);
console.log(
  `${route}: hash=${state.hash} h1="${state.title}" ready=${state.ready} viewport=${state.width}x${state.height}`,
);

/*
 * PROBELAB_LAYOUT=1 reports the numbers a screenshot cannot: which element is wider than the
 * viewport, and by how much. Redesigning a native window means reading the capture, finding that
 * the right-hand column is missing, and then still having to guess whether the grid, the scroll
 * container, or a single child is the thing that refused to shrink. This prints the answer.
 */
if (process.env.PROBELAB_LAYOUT === "1") {
  const { result: layoutResult } = await send("Runtime.evaluate", {
    expression: `JSON.stringify((() => {
      const doc = document.documentElement;
      const overflowing = [...document.querySelectorAll("*")]
        .filter((node) => node.getBoundingClientRect().right > doc.clientWidth + 1)
        .slice(0, 8)
        .map((node) => {
          const box = node.getBoundingClientRect();
          return node.className + " @" + Math.round(box.left) + ".." + Math.round(box.right);
        });
      return {
        viewport: doc.clientWidth + "x" + doc.clientHeight,
        scrollWidth: doc.scrollWidth,
        scrollHeight: doc.scrollHeight,
        overflowing,
      };
    })())`,
    returnByValue: true,
  });
  console.log(layoutResult.value);
}

if (consoleErrors.length > 0) {
  for (const line of consoleErrors) console.error(`  console.error: ${line}`);
}

socket.close();
process.exit(consoleErrors.length > 0 ? 1 : 0);
