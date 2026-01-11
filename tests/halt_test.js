handle {
    perform Print!("before")
    perform Exit!(42)
    perform Print!("after - should not print")
} with {
    Exit!(code, resume) -> {
        perform Print!("exiting with code", code)
        code  // return value, don't call resume
    }
}
