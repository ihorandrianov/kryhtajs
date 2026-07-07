// Joining a failed fiber yields {err: reason} — a value to match on,
// not an exception.
let f = perform Fork!(function() {
    boom();
});
let g = perform Fork!(function() {
    return 0;
});

// Let the crasher run to failure.
perform Join!(g);

let r = perform Join!(f);
match (r) {
    {ok} => false,
    {err} => true
}
