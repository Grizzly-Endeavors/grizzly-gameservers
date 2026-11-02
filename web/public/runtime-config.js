// Settings the deployed console receives when its pod starts rather than at
// build time. nginx serves its own version of this file in the cluster
// (web/nginx.conf.template); this empty one is what `npm run dev` serves, where
// the channel comes from web/.env instead.
window.__GARY__ = {};
