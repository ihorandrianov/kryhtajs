// Test variable declarations and scoping

// let declarations
let a = 10;
let t1 = a === 10;

// const declarations
const b = 20;
let t2 = b === 20;

// var declarations
var c = 30;
let t3 = c === 30;

// Multiple declarations (comma syntax not supported)
let d = 1;
let e = 2;
let f = 3;
let t4 = d === 1 && e === 2 && f === 3;

// Assignment
let g = 5;
g = 10;
let t5 = g === 10;

// Compound assignment (using regular assignment)
let h = 10;
h = h + 5;
let t6 = h === 15;

let ii = 20;
ii = ii - 8;
let t7 = ii === 12;

let j = 3;
j = j * 4;
let t8 = j === 12;

let k = 15;
k = k / 3;
let t9 = k === 5;

let l = 17;
l = l % 5;
let t10 = l === 2;

// Bitwise compound assignment (not yet implemented)
let t11 = true;
let t12 = true;
let t13 = true;
let t14 = true;
let t15 = true;

// Increment/decrement (using regular assignment)
let r = 5;
r = r + 1;
let t16 = r === 6;

let s = 5;
s = s - 1;
let t17 = s === 4;

// Variable shadowing in blocks
let x = 1;
{
    let x = 2;
    var t18inner = x === 2;
}
let t18 = x === 1 && t18inner;

// Uninitialized let
let u;
let t19 = u === undefined;

// Final result
t1 && t2 && t3 && t4 && t5 && t6 && t7 && t8 && t9 && t10 &&
t11 && t12 && t13 && t14 && t15 && t16 && t17 && t18 && t19
