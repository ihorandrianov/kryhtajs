// Test Math built-in functions

// Math.floor
let t1 = Math.floor(3.7) === 3;
let t2 = Math.floor(-3.2) === -4;
let t3 = Math.floor(5) === 5;

// Math.ceil
let t4 = Math.ceil(3.2) === 4;
let t5 = Math.ceil(-3.7) === -3;
let t6 = Math.ceil(5) === 5;

// Math.round
let t7 = Math.round(3.4) === 3;
let t8 = Math.round(3.5) === 4;
let t9 = Math.round(-3.5) === -3;

// Math.abs
let t10 = Math.abs(5) === 5;
let t11 = Math.abs(-5) === 5;
let t12 = Math.abs(0) === 0;

// Math.sqrt
let t13 = Math.sqrt(4) === 2;
let t14 = Math.sqrt(9) === 3;
let t15 = Math.sqrt(2) > 1.41 && Math.sqrt(2) < 1.42;

// Math.pow (if available, otherwise use **)
let t16 = Math.pow(2, 3) === 8;
let t17 = Math.pow(5, 2) === 25;
let t18 = Math.pow(2, 10) === 1024;

// Math.min / Math.max
let t19 = Math.min(3, 7) === 3;
let t20 = Math.max(3, 7) === 7;
let t21 = Math.min(-5, -2) === -5;
let t22 = Math.max(-5, -2) === -2;

// Math.sin / Math.cos (approximate checks)
let t23 = Math.sin(0) === 0;
let t24 = Math.cos(0) === 1;
let t25 = Math.sin(Math.PI / 2) > 0.99 && Math.sin(Math.PI / 2) < 1.01;

// Math.PI and Math.E constants
let t26 = Math.PI > 3.14 && Math.PI < 3.15;
let t27 = Math.E > 2.71 && Math.E < 2.72;

// Math.log
let t28 = Math.log(1) === 0;
let t29 = Math.log(Math.E) > 0.99 && Math.log(Math.E) < 1.01;

// Math.exp
let t30 = Math.exp(0) === 1;
let t31 = Math.exp(1) > 2.71 && Math.exp(1) < 2.72;

// Math.trunc
let t32 = Math.trunc(3.7) === 3;
let t33 = Math.trunc(-3.7) === -3;

// Math.sign
let t34 = Math.sign(5) === 1;
let t35 = Math.sign(-5) === -1;
let t36 = Math.sign(0) === 0;

// Chained operations (using intermediate vars due to nested call bug)
let sqrt10 = Math.sqrt(10);
let t37 = Math.floor(sqrt10) === 3;
let floorNeg = Math.floor(-2.5);
let t38 = Math.abs(floorNeg) === 3;

// Final result
t1 && t2 && t3 && t4 && t5 && t6 && t7 && t8 && t9 && t10 &&
t11 && t12 && t13 && t14 && t15 && t16 && t17 && t18 && t19 && t20 &&
t21 && t22 && t23 && t24 && t25 && t26 && t27 && t28 && t29 && t30 &&
t31 && t32 && t33 && t34 && t35 && t36 && t37 && t38
