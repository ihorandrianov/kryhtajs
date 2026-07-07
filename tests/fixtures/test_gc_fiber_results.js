// A completed fiber's result may live only in its FiberStatus until joined.
// It must survive a collection that happens in that window.

let a = perform Fork!(function() {
    return { answer: 42 };
});
let b = perform Fork!(function() {
    return 0;
});

// Block on b: scheduler runs a to completion (result parked in its status),
// then b, then resumes us.
perform Join!(b);

perform Gc!();

// Churn allocations so any wrongly-freed slot gets reused/overwritten.
let i = 0;
while (i < 100) {
    let o = { junk: i };
    i = i + 1;
}

let r = perform Join!(a);
match (r) {
    {ok} => ok.answer === 42,
    {err} => false
}
