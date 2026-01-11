// Test control flow statements

// if statement
let t1 = false;
if (true) {
    t1 = true;
}

// if-else
let t2 = false;
if (false) {
    t2 = false;
} else {
    t2 = true;
}

// if-else if-else
let t3 = false;
let val = 2;
if (val === 1) {
    t3 = false;
} else if (val === 2) {
    t3 = true;
} else {
    t3 = false;
}

// Nested if
let t4 = false;
if (true) {
    if (true) {
        t4 = true;
    }
}

// while loop
let sum = 0;
let counter = 1;
while (counter <= 5) {
    sum = sum + counter;
    counter = counter + 1;
}
let t5 = sum === 15;

// while with break
let t6val = 0;
while (true) {
    t6val = t6val + 1;
    if (t6val === 5) {
        break;
    }
}
let t6 = t6val === 5;

// while with continue
let t7sum = 0;
let t7i = 0;
while (t7i < 10) {
    t7i = t7i + 1;
    if (t7i % 2 === 0) {
        continue;
    }
    t7sum = t7sum + t7i;
}
let t7 = t7sum === 25;

// for loop
let t8sum = 0;
for (let i = 1; i <= 5; i = i + 1) {
    t8sum = t8sum + i;
}
let t8 = t8sum === 15;

// for loop with break
let t9val = 0;
for (let i = 0; i < 100; i = i + 1) {
    t9val = i;
    if (i === 10) {
        break;
    }
}
let t9 = t9val === 10;

// for loop with continue
let t10sum = 0;
for (let i = 1; i <= 10; i = i + 1) {
    if (i % 2 === 0) {
        continue;
    }
    t10sum = t10sum + i;
}
let t10 = t10sum === 25;

// Nested loops
let t11val = 0;
for (let i = 0; i < 3; i = i + 1) {
    for (let j = 0; j < 3; j = j + 1) {
        t11val = t11val + 1;
    }
}
let t11 = t11val === 9;

// Nested loops with break (inner only)
let t12val = 0;
for (let i = 0; i < 3; i = i + 1) {
    for (let j = 0; j < 10; j = j + 1) {
        if (j === 2) {
            break;
        }
        t12val = t12val + 1;
    }
}
let t12 = t12val === 6;

// Empty for loop parts
let t13i = 0;
for (; t13i < 5;) {
    t13i = t13i + 1;
}
let t13 = t13i === 5;

// Falsy conditions
let t14 = true;
if (0) { t14 = false; }
if ("") { t14 = false; }
if (null) { t14 = false; }
if (undefined) { t14 = false; }

// Truthy conditions
let t15 = false;
if (1) {
    if ("a") {
        t15 = true;
    }
}

t1 && t2 && t3 && t4 && t5 && t6 && t7 && t8 && t9 && t10 &&
t11 && t12 && t13 && t14 && t15
