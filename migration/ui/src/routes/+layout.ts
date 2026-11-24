// adapter-static + no prerender: every path is served by axum as the same shell
// document, with window.ENV injected per request. Nothing here can run on a
// server, because there is no server.
export const ssr = false;
export const prerender = false;
export const csr = true;
