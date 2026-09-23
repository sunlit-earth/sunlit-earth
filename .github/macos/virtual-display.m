// Adds one virtual screen to this session for as long as it runs, through
// CoreGraphics' private CGVirtualDisplay, the API DeskPad and BetterDisplay
// use. The interfaces below are the subset of the class dumps it needs; the
// classes are looked up at runtime, so nothing links against a private symbol.
//
//   virtual-display <width> <height>

#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>

@interface CGVirtualDisplayMode : NSObject
- (instancetype)initWithWidth:(NSUInteger)width height:(NSUInteger)height refreshRate:(CGFloat)rate;
@end

@interface CGVirtualDisplaySettings : NSObject
@property(retain, nonatomic) NSArray *modes;
@property(nonatomic) unsigned int hiDPI;
@end

@interface CGVirtualDisplayDescriptor : NSObject
@property(retain, nonatomic) NSString *name;
@property(nonatomic) unsigned int maxPixelsHigh;
@property(nonatomic) unsigned int maxPixelsWide;
@property(nonatomic) CGSize sizeInMillimeters;
@property(nonatomic) unsigned int serialNum;
@property(nonatomic) unsigned int productID;
@property(nonatomic) unsigned int vendorID;
- (void)setDispatchQueue:(dispatch_queue_t)queue;
@end

@interface CGVirtualDisplay : NSObject
@property(readonly, nonatomic) CGDirectDisplayID displayID;
- (instancetype)initWithDescriptor:(CGVirtualDisplayDescriptor *)descriptor;
- (BOOL)applySettings:(CGVirtualDisplaySettings *)settings;
@end

static void print_displays(void) {
    CGDirectDisplayID ids[16];
    uint32_t count = 0;
    CGGetActiveDisplayList(16, ids, &count);
    printf("active displays: %u\n", count);
    for (uint32_t i = 0; i < count; i++) {
        CGRect b = CGDisplayBounds(ids[i]);
        printf("  [%u] id %u at %.0f,%.0f size %.0fx%.0f%s\n", i, ids[i], b.origin.x, b.origin.y,
               b.size.width, b.size.height, CGDisplayIsMain(ids[i]) ? " main" : "");
    }
    fflush(stdout);
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        if (argc != 3) {
            fprintf(stderr, "usage: %s <width> <height>\n", argv[0]);
            return 2;
        }
        unsigned int width = (unsigned int)atoi(argv[1]);
        unsigned int height = (unsigned int)atoi(argv[2]);

        Class descriptorClass = NSClassFromString(@"CGVirtualDisplayDescriptor");
        Class displayClass = NSClassFromString(@"CGVirtualDisplay");
        Class settingsClass = NSClassFromString(@"CGVirtualDisplaySettings");
        Class modeClass = NSClassFromString(@"CGVirtualDisplayMode");
        if (!descriptorClass || !displayClass || !settingsClass || !modeClass) {
            fprintf(stderr, "this macOS has no CGVirtualDisplay\n");
            return 1;
        }

        CGVirtualDisplayDescriptor *descriptor = [[descriptorClass alloc] init];
        [descriptor setDispatchQueue:dispatch_get_main_queue()];
        descriptor.name = @"Sunlit Earth test screen";
        descriptor.maxPixelsWide = width;
        descriptor.maxPixelsHigh = height;
        // A 24-inch panel; macOS derives the reported DPI from this.
        descriptor.sizeInMillimeters = CGSizeMake(530, 300);
        descriptor.vendorID = 0x5345;
        descriptor.productID = 0x0002;
        descriptor.serialNum = 2;

        // The screen goes when this object does, and ARC may release a local
        // after its last use even though main never returns.
        __attribute__((objc_precise_lifetime)) CGVirtualDisplay *display =
            [[displayClass alloc] initWithDescriptor:descriptor];
        if (!display || display.displayID == 0) {
            fprintf(stderr, "CGVirtualDisplay refused the descriptor\n");
            return 1;
        }

        CGVirtualDisplaySettings *settings = [[settingsClass alloc] init];
        settings.hiDPI = 0;
        settings.modes = @[ [[modeClass alloc] initWithWidth:width height:height refreshRate:60] ];
        if (![display applySettings:settings]) {
            fprintf(stderr, "CGVirtualDisplay refused the %ux%u mode\n", width, height);
            return 1;
        }
        printf("virtual display %u is %ux%u\n", display.displayID, width, height);

        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 3 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
            print_displays();
        });
        dispatch_main();
    }
}
