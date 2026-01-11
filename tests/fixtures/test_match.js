// Test pattern matching

// Literal matching
let t1 = match (1) { 1 => true, _ => false };
let t2 = match (2) { 1 => false, 2 => true, _ => false };
let t3 = match ("hello") { "hello" => true, _ => false };

// Wildcard
let t4 = match (42) { _ => true };

// Variable binding
let t5 = match (5) { x => x === 5 };
let t6 = match (10) { n => n * 2 } === 20;

// Multiple arms
function grade(score) {
    return match (score) {
        100 => "perfect",
        _ => "other"
    };
}
let t7 = grade(100) === "perfect" && grade(50) === "other";

// Match in recursion
function factorial(n) {
    return match (n) {
        0 => 1,
        1 => 1,
        x => x * factorial(x - 1)
    };
}
let t8 = factorial(5) === 120;

// Array destructuring
let t9 = match ([1, 2]) { [a, b] => a + b } === 3;
let t10 = match ([1, 2, 3]) { [x, y, z] => x + y + z } === 6;

// Object destructuring
let t11 = match ({x: 10, y: 20}) { {x, y} => x + y } === 30;
let t12 = match ({a: 1, b: 2, c: 3}) { {a, c} => a + c } === 4;

// Nested patterns
let t13 = match ({point: {x: 3, y: 4}}) { {point: {x, y}} => x + y } === 7;
let t14 = match ([[1, 2], [3, 4]]) { [[a, b], [c, d]] => a + b + c + d } === 10;

// Mixed array/object
let t15 = match ([{x: 1}, {x: 2}]) { [{x: a}, {x: b}] => a + b } === 3;

t1 && t2 && t3 && t4 && t5 && t6 && t7 && t8 && t9 && t10 && t11 && t12 && t13 && t14 && t15
