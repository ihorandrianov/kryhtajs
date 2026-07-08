// Snapshot mid-run with fibers in every state, then verify the world
// is intact: closure state, heap objects, blocked and unjoined fibers.
var acc = { total: 0 };

function mkAdder(n) {
    return function() { return n + acc.total };
}
var add10 = mkAdder(10);

// completed-but-unjoined fiber: its result sits parked in its status
var parked = perform Fork!(function() { return { answer: 32 } });
var quick = perform Fork!(function() { return 0 });
perform Join!(quick);

acc.total = 10;

perform Snapshot!("__SNAP_PATH__");

// everything below runs in BOTH worlds and must agree
let joined = perform Join!(parked);
let fromFiber = match (joined) { {ok} => ok.answer, {err} => -1 };
add10() + fromFiber   // 10 + 10 + 32 = 52
