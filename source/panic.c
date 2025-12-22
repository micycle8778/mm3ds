#include <3ds.h>
#include <stdlib.h>

void
panic(const char* message) {
    errorConf err;
    errorInit(&err, ERROR_TEXT_WORD_WRAP, 0);
    errorText(&err, message);
    errorDisp(&err);

    // svcBreak(0);
    abort();
}
