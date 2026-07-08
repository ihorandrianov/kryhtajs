// Durable counter: run it, kill it, resume it — it continues.
//
//   cargo run --bin kryhta examples/durable_counter.js
//   cargo run --bin kryhta -- --resume counter.snap
var state = { i: 0 };

while (state.i < 5) {
    state.i = state.i + 1;
    let outcome = perform Snapshot!("counter.snap");
    perform Print!(outcome, "i =", state.i);
}
state.i
