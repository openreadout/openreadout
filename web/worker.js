// The demo's Web Worker: OpenReadout runs here, off the page's thread (core.js).
import { createCore } from "./core.js";

const core = createCore((message, transfer) => postMessage(message, transfer));
addEventListener("message", (e) => core.receive(e.data));
postMessage({ hello: true });
