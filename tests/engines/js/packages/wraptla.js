// A CommonJS module that requires one with top-level await: it runs once, then fails.
console.log("wraptla.js runs");
module.exports = { val: require("./tla.mjs").N };
