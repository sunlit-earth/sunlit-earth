// Prints each active screen's size as <width>x<height>, one line per screen in
// CGGetActiveDisplayList order, the order macVNC's -display index counts in,
// and through the same calls macVNC sizes its framebuffer with.

#include <CoreGraphics/CoreGraphics.h>
#include <stdio.h>

int main(void) {
    CGDirectDisplayID ids[32];
    uint32_t count = 0;
    if (CGGetActiveDisplayList(32, ids, &count) != kCGErrorSuccess) {
        return 1;
    }
    for (uint32_t i = 0; i < count; i++) {
        printf("%zux%zu\n", CGDisplayPixelsWide(ids[i]), CGDisplayPixelsHigh(ids[i]));
    }
    return 0;
}
