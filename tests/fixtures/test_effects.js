// Test algebraic effects

// Basic effect with resume
let t1 = handle {
    perform Get!()
} with {
    Get!(resume) -> resume(42)
} === 42;

// Effect with argument
let t2 = handle {
    perform Double!(21)
} with {
    Double!(x, resume) -> resume(x * 2)
} === 42;

// Multiple performs
let t3 = handle {
    let a = perform Get!();
    let b = perform Get!();
    a + b
} with {
    Get!(resume) -> resume(10)
} === 20;

// Sequential effects
let t4 = handle {
    let x = perform Get!();
    perform Put!(x + 1);
    perform Get!()
} with {
    Get!(resume) -> resume(5),
    Put!(v, resume) -> resume(v * 2)
} === 5;

// Return clause
let t5 = handle {
    perform Get!()
} with {
    Get!(resume) -> resume(10),
    return(x) -> x * 2
} === 20;

// Effect in function
function useEffect() {
    return perform Get!();
}
let t6 = handle {
    useEffect() + useEffect()
} with {
    Get!(resume) -> resume(7)
} === 14;

// Nested handle
let t7 = handle {
    handle {
        perform Inner!()
    } with {
        Inner!(resume) -> resume(perform Outer!())
    }
} with {
    Outer!(resume) -> resume(100)
} === 100;

// Effect with multiple args
let t8 = handle {
    perform Add!(3, 4)
} with {
    Add!(a, b, resume) -> resume(a + b)
} === 7;

// State-like pattern
let t9 = handle {
    perform Inc!();
    perform Inc!();
    perform Inc!();
    perform Read!()
} with {
    Inc!(resume) -> (s) => resume(undefined)(s + 1),
    Read!(resume) -> (s) => resume(s)(s),
    return(x) -> (s) => x
}(0) === 3;

// Mutable cell pattern
let t10 = handle {
    perform Set!(10);
    let x = perform Read!();
    perform Set!(x * 2);
    perform Read!()
} with {
    Set!(v, resume) -> (s) => resume(undefined)(v),
    Read!(resume) -> (s) => resume(s)(s),
    return(x) -> (s) => x
}(0) === 20;

t1 && t2 && t3 && t4 && t5 && t6 && t7 && t8 && t9 && t10
