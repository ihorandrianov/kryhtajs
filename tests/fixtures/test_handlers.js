// Test first-class handlers

// Basic handler as value
let h1 = handler {
    Get!(resume) -> resume(42)
};
let t1 = handle { perform Get!() } with h1 === 42;

// Handler with return clause
let h2 = handler {
    Get!(resume) -> resume(10),
    return(x) -> x * 2
};
let t2 = handle { perform Get!() } with h2 === 20;

// Handler captures environment (closure)
// Use function to properly capture value (JS closures capture variables, not values)
function makeH3(captured) {
    return handler {
        Get!(resume) -> resume(captured)
    };
}
let h3 = makeH3(100);
let t3 = handle { perform Get!() } with h3 === 100;

// Reuse handler multiple times
let h4 = handler {
    Inc!(resume) -> resume(1)
};
let r1 = handle { perform Inc!() } with h4;
let r2 = handle { perform Inc!() } with h4;
let t4 = r1 === 1 && r2 === 1;

// Handler passed to function
let h5 = handler {
    Get!(resume) -> resume(7)
};
function runWith(h) {
    return handle { perform Get!() + perform Get!() } with h;
}
let t5 = runWith(h5) === 14;

// Handler with multiple effects
let h6 = handler {
    Get!(resume) -> resume(5),
    Put!(v, resume) -> resume(v * 2)
};
let t6 = handle {
    let a = perform Get!();
    perform Put!(a)
} with h6 === 10;

// Handler stored in object
let obj = {
    h: handler {
        Get!(resume) -> resume(33)
    }
};
let t7 = handle { perform Get!() } with obj.h === 33;

// Handler stored in array
let arr = [handler { Get!(resume) -> resume(44) }];
let t8 = handle { perform Get!() } with arr[0] === 44;

// Nested captures - use function to capture value
function makeH9(capturedA) {
    return handler {
        Get!(resume) -> {
            let b = 2;
            resume(capturedA + b)
        }
    };
}
let h9 = makeH9(1);
let t9 = handle { perform Get!() } with h9 === 3;

// Handler with multi-arg effect
let h10 = handler {
    Add!(x, y, resume) -> resume(x + y)
};
let t10 = handle { perform Add!(10, 20) } with h10 === 30;

t1 && t2 && t3 && t4 && t5 && t6 && t7 && t8 && t9 && t10
