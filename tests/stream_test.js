// Simple generator pattern with Yield! effect

// Producer function
function numbers(n) {
    let i = 0
    while (i < n) {
        perform Yield!(i)
        i = i + 1
    }
}

// Consumer handles yields
perform Print!("Basic stream:")
handle {
    numbers(5)
} with {
    Yield!(value, resume) -> {
        perform Print!("got", value)
        resume(undefined)
    }
}

// Transform: map
perform Print!("Mapped stream (x * 2):")
handle {
    handle {
        numbers(5)
    } with {
        Yield!(value, resume) -> {
            perform Yield!(value * 2)
            resume(undefined)
        }
    }
} with {
    Yield!(value, resume) -> {
        perform Print!("got", value)
        resume(undefined)
    }
}

// Transform: filter
perform Print!("Filtered stream (x > 2):")
handle {
    handle {
        numbers(5)
    } with {
        Yield!(value, resume) -> {
            if (value > 2) {
                perform Yield!(value)
            }
            resume(undefined)
        }
    }
} with {
    Yield!(value, resume) -> {
        perform Print!("got", value)
        resume(undefined)
    }
}

// Early termination - stop after 3
perform Print!("Early stop after 3:")
handle {
    numbers(100)
} with {
    Yield!(value, resume) -> {
        perform Print!("got", value)
        if (value < 3) {
            resume(undefined)
        }
        // else don't resume - stops the stream
    }
}

perform Print!("done")
