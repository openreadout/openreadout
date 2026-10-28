// The demo's Web Worker: OpenReadout runs here, off the page's thread (core.js). Requests to
// other sites go through the page (engine.js), which some servers answer more readily.
import { createCore } from "./core.js";
import { fetchWith } from "./store.js";

const core = createCore((message, transfer) => postMessage(message, transfer));
const fetches = new Map();
let nextFetch = 1;
fetchWith(
  (at, options, body) =>
    new Promise((resolve, reject) => {
      const id = nextFetch++;
      fetches.set(id, { resolve, reject });
      postMessage({ fetch: { id, at, options, body } });
    }),
);
addEventListener("message", (e) => {
  const f = e.data.fetched;
  if (!f) return core.receive(e.data);
  const p = fetches.get(f.id);
  fetches.delete(f.id);
  if (f.error) p.reject(Object.assign(new Error(f.error.message), { name: f.error.name }));
  else p.resolve({ ...f.response, headers: new Map(Object.entries(f.response.headers)) });
});
postMessage({ hello: true });
