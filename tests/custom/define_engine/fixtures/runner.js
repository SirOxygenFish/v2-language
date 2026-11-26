const src = require("fs").readFileSync(process.argv[2], "utf8");
console.log("runner.js ran: " + src.trim());
