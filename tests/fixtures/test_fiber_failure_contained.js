// A crashed child fiber must not take down the whole runtime.
perform Fork!(function() {
    boom();
});

let f = perform Fork!(function() {
    return 5;
});

// Blocking here lets the crasher run (and fail) before we resume.
let r = perform Join!(f);
match (r) {
    {ok} => ok === 5,
    {err} => false
}
