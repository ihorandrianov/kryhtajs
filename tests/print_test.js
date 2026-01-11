perform Print!("hello", "world")
perform Print!(1, 2, 3)
perform Print!("result:", 1 + 2 * 3)

let x = 42
perform Print!("x is", x)

// Test shadowing
handle {
    perform Print!("this is shadowed")
} with {
    Print!(msg, resume) -> {
        perform Print!("[CUSTOM]", msg)
        resume(undefined)
    }
}
