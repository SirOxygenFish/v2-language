function Fn() { return "fn"; }
Fn.helper = () => "helper";
Fn.tag = "tagged";
function Point(x, y) { this.x = x; this.y = y; }
function whoami() { return this === module.exports ? "the module" : "someone else"; }
module.exports = { Fn, Point, whoami };
