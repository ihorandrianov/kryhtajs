// A forked function body must run in a fresh frame — its declarations
// must not leak into the captured (parent) environment.
let x = 1;

let f = perform Fork!(function() {
    let x = 99;
    return x;
});

let r = perform Join!(f);

match (r) {
    {ok} => ok === 99 && x === 1,
    {err} => false
}
