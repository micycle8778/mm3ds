#pragma once

void panic(const char* message);

#define STR2(x) #x
#define STR(x) STR2(x)

#define PANIC(message) \
    do { \
        panic(__FILE__ ":" STR(__LINE__) ": " message); \
    } while (0)
#define PANIC_IF_NULL(value) \
    if (value == NULL) { \
        PANIC(#value " was null!"); \
    }
#define PANIC_IF_ZERO(value) \
    if (value == 0) { \
        PANIC(#value " was zero!"); \
    }
