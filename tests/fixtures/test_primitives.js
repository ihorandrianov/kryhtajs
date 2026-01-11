// Test primitive values
// Each test assigns a boolean, final expression is AND of all tests

// Undefined
let t1 = undefined === undefined;

// Null
let t2 = null === null;

// Booleans
let t3 = true === true;
let t4 = false === false;
let t5 = true !== false;

// Integers
let t6 = 0 === 0;
let t7 = 42 === 42;
let t8 = -1 === -1;
let t9 = 123456 === 123456;

// Floats
let t10 = 3.14 === 3.14;
let t11 = -0.5 === -0.5;
let t12 = 1.5e10 === 1.5e10;

// Strings
let t13 = "hello" === "hello";
let t14 = 'world' === 'world';
let t15 = "" === "";

// Type distinctions
let t16 = null !== undefined;
let t17 = 0 !== false;
let t18 = "" !== false;
let t19 = 1 !== true;

// Final result - all tests must pass
t1 && t2 && t3 && t4 && t5 && t6 && t7 && t8 && t9 && t10 && t11 && t12 && t13 && t14 && t15 && t16 && t17 && t18 && t19
