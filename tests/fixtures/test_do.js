// Test do block expressions

// Basic do block
let t1 = do { 42 } === 42;

// Do with statements
let t2 = do {
    let a = 1;
    let b = 2;
    a + b
} === 3;

// Do with loop
let t3 = do {
    let sum = 0;
    let i = 1;
    while (i <= 5) {
        sum = sum + i;
        i = i + 1;
    }
    sum
} === 15;

// Nested do
let t4 = do {
    let inner = do {
        let x = 10;
        x * 2
    };
    inner + 5
} === 25;

// Do in function
function compute(n) {
    return do {
        let result = 1;
        let i = 1;
        while (i <= n) {
            result = result * i;
            i = i + 1;
        }
        result
    };
}
let t5 = compute(5) === 120;

// Do as argument
function double(x) { return x * 2; }
let t6 = double(do { 3 + 4 }) === 14;

// Do with conditional
let t7 = do {
    let x = 10;
    if (x > 5) {
        x = x * 2;
    }
    x
} === 20;

// Do returning object/array
let t8 = do {
    let obj = {a: 1, b: 2};
    obj.a + obj.b
} === 3;

let t9 = do {
    let arr = [1, 2, 3];
    arr[0] + arr[2]
} === 4;

// Complex nesting
let t10 = do {
    let result = 0;
    for (let i = 0; i < 3; i++) {
        result = result + do {
            let inner = i * 2;
            inner + 1
        };
    }
    result
} === 9;

t1 && t2 && t3 && t4 && t5 && t6 && t7 && t8 && t9 && t10
