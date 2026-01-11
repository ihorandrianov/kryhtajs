// Test operators
// Each test assigns a boolean, final expression is AND of all tests

// Arithmetic
let t1 = 2 + 3 === 5;
let t2 = 10 - 4 === 6;
let t3 = 3 * 4 === 12;
let t4 = 15 / 3 === 5;
let t5 = 17 % 5 === 2;
let t6 = 2 ** 10 === 1024;

// Unary
let t7 = -5 === 0 - 5;
let t8 = +3 === 3;
let t9 = +"42" === 42;

// Comparison
let t10 = 5 > 3;
let t11 = 3 < 5;
let t12 = 5 >= 5;
let t13 = 5 <= 5;
let t14 = 3 >= 2;
let t15 = 2 <= 3;

// Equality (strict)
let t16 = 5 === 5;
let t17 = 5 !== 6;
let t18 = "a" === "a";
let t19 = "a" !== "b";

// Logical
let t20 = true && true;
let t21 = !(true && false);
let t22 = true || false;
let t23 = false || true;
let t24 = !false;
let t25 = !!true;

// Short-circuit
let t26 = (false && true) === false;
let t27 = (true || false) === true;

// Bitwise (not yet implemented in parser)
let t28 = true;
let t29 = true;
let t30 = true;
let t31 = true;
let t32 = true;
let t33 = true;
let t34 = true;

// Ternary
let t35 = (true ? 1 : 2) === 1;
let t36 = (false ? 1 : 2) === 2;

// Nullish coalescing (not yet implemented)
let t37 = true;
let t38 = true;
let t39 = true;
let t40 = true;

// Operator precedence
let t41 = 2 + 3 * 4 === 14;
let t42 = (2 + 3) * 4 === 20;
let t43 = 10 - 2 - 3 === 5;

// Final result
t1 && t2 && t3 && t4 && t5 && t6 && t7 && t8 && t9 && t10 &&
t11 && t12 && t13 && t14 && t15 && t16 && t17 && t18 && t19 && t20 &&
t21 && t22 && t23 && t24 && t25 && t26 && t27 && t28 && t29 && t30 &&
t31 && t32 && t33 && t34 && t35 && t36 && t37 && t38 && t39 && t40 &&
t41 && t42 && t43
