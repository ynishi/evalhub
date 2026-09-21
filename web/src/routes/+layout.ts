// A single-page application: no server rendering, no prerendering. The
// hub serves one `index.html` for every browser path and this router takes
// it from there.
export const ssr = false;
export const prerender = false;
export const trailingSlash = 'never';
