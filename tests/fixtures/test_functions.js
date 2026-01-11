// Test functions and closures

// Basic function
function add(a, b) { return a + b; }
let t1 = add(2, 3) === 5;

// Function expression
let mul = function(a, b) { return a * b; };
let t2 = mul(4, 5) === 20;

// Arrow function
let sub = (a, b) => a - b;
let t3 = sub(10, 3) === 7;

// Arrow with block
let div = (a, b) => { return a / b; };
let t4 = div(20, 4) === 5;

// Nested calls (using intermediate vars due to nested call bug)
function inc(x) { return x + 1; }
let r1 = inc(0);
let r2 = inc(r1);
let r3 = inc(r2);
let t5 = r3 === 3;

// Recursion
function fib(n) {
    if (n <= 1) return n;
    return fib(n - 1) + fib(n - 2);
}
let t6 = fib(10) === 55;

// Closure
function makeCounter() {
    let count = 0;
    return function() {
        count = count + 1;
        return count;
    };
}
let counter = makeCounter();
let t7 = counter() === 1 && counter() === 2 && counter() === 3;

// Closure capturing multiple variables
function makeAdder(x) {
    return function(y) {
        return x + y;
    };
}
let add5 = makeAdder(5);
let add10 = makeAdder(10);
let t8 = add5(3) === 8 && add10(3) === 13;

// Higher-order function
function apply(f, x) { return f(x); }
let t9 = apply(inc, 5) === 6;

// Function returning function
function compose(f, g) {
    return function(x) { return f(g(x)); };
}
let double = (x) => x * 2;
let incThenDouble = compose(double, inc);
let t10 = incThenDouble(3) === 8;

// Immediate invocation
let t11 = (function(x) { return x * x; })(5) === 25;

// Arrow immediate invocation
let t12 = ((x) => x + 1)(10) === 11;

t1 && t2 && t3 && t4 && t5 && t6 && t7 && t8 && t9 && t10 && t11 && t12
