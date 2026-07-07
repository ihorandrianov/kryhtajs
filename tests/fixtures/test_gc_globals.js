// Top-level functions live in globals and the global env — must survive collection.
function inc(x) {
    return x + 1;
}

// `var` declarations live ONLY in the globals map (no env binding),
// so they are reachable through no other GC root.
var boxed = { answer: 1 };

perform Gc!();

// Churn allocations so any wrongly-freed slot gets reused/overwritten.
let i = 0;
let acc = 0;
while (i < 100) {
    let o = { junk: i };
    acc = acc + o.junk;
    i = i + 1;
}

inc(41) === 42 && boxed.answer === 1
