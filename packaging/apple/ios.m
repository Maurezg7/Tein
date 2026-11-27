#import <AVFoundation/AVFoundation.h>
#import <MediaPlayer/MediaPlayer.h>
#import <UIKit/UIKit.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>
#include <stdbool.h>
#include <math.h>

extern void tein_command(int64_t action);
extern void tein_selected(bool image, const char *path);

@interface TeinPicker : NSObject <UIDocumentPickerDelegate, UIAdaptivePresentationControllerDelegate>
@property(nonatomic) BOOL image;
@end
static TeinPicker *pickerDelegate;

@implementation TeinPicker
- (void)finish:(NSURL *)url {
    if (pickerDelegate != self) return;
    tein_selected(self.image, url.fileSystemRepresentation);
    pickerDelegate = nil;
}
- (void)documentPickerWasCancelled:(UIDocumentPickerViewController *)controller { [self finish:nil]; }
- (void)presentationControllerDidDismiss:(UIPresentationController *)controller { [self finish:nil]; }
- (void)documentPicker:(UIDocumentPickerViewController *)controller didPickDocumentsAtURLs:(NSArray<NSURL *> *)urls {
    NSURL *url = urls.firstObject;
    if (!self.image || !url) { [self finish:url]; return; }
    // The Rust image editor deletes its input: always pass a private copy.
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        BOOL scoped = [url startAccessingSecurityScopedResource];
        __block NSURL *copy = nil;
        NSFileCoordinator *coordinator = [[NSFileCoordinator alloc] initWithFilePresenter:nil];
        [coordinator coordinateReadingItemAtURL:url options:0 error:nil byAccessor:^(NSURL *source) {
            NSNumber *regular, *size;
            if (![source getResourceValue:&regular forKey:NSURLIsRegularFileKey error:nil] || !regular.boolValue ||
                ![source getResourceValue:&size forKey:NSURLFileSizeKey error:nil] || size.unsignedLongLongValue > 32ULL * 1024 * 1024) return;
            NSURL *dst = [[NSURL fileURLWithPath:NSTemporaryDirectory()] URLByAppendingPathComponent:NSUUID.UUID.UUIDString];
            if ([[NSFileManager defaultManager] copyItemAtURL:source toURL:dst error:nil]) copy = dst;
        }];
        if (scoped) [url stopAccessingSecurityScopedResource];
        dispatch_async(dispatch_get_main_queue(), ^{ [self finish:copy]; });
    });
}
@end

bool tein_ios_pick(bool image) {
    if (!NSThread.isMainThread || pickerDelegate) return false;
    UIViewController *root = nil;
    for (UIScene *scene in UIApplication.sharedApplication.connectedScenes) {
        if (![scene isKindOfClass:UIWindowScene.class] || scene.activationState != UISceneActivationStateForegroundActive) continue;
        for (UIWindow *window in ((UIWindowScene *)scene).windows)
            if (window.isKeyWindow) root = window.rootViewController;
    }
    if (!root || root.presentedViewController) return false;
    UIDocumentPickerViewController *picker = [[UIDocumentPickerViewController alloc]
        initForOpeningContentTypes:@[image ? UTTypeImage : UTTypeFolder] asCopy:NO];
    if (!image) picker.directoryURL = [[NSFileManager defaultManager] URLsForDirectory:NSDocumentDirectory inDomains:NSUserDomainMask].firstObject;
    pickerDelegate = [TeinPicker new];
    pickerDelegate.image = image;
    picker.delegate = pickerDelegate;
    picker.presentationController.delegate = pickerDelegate;
    [root presentViewController:picker animated:YES completion:nil];
    return true;
}

static void bind(MPRemoteCommand *command, int64_t action) {
    [command addTargetWithHandler:^MPRemoteCommandHandlerStatus(MPRemoteCommandEvent *event) {
        tein_command(action);
        return MPRemoteCommandHandlerStatusSuccess;
    }];
}
void tein_ios_init(void) {
    [AVAudioSession.sharedInstance setCategory:AVAudioSessionCategoryPlayback error:nil];
    [NSNotificationCenter.defaultCenter addObserverForName:UIApplicationDidBecomeActiveNotification object:nil queue:NSOperationQueue.mainQueue usingBlock:^(NSNotification *note) {
        tein_command(10); // Rescan files copied while Tein was in the background.
        static dispatch_once_t once;
        dispatch_once(&once, ^{
            MPRemoteCommandCenter *c = MPRemoteCommandCenter.sharedCommandCenter;
            bind(c.togglePlayPauseCommand, 1); bind(c.nextTrackCommand, 2); bind(c.previousTrackCommand, 3);
            bind(c.pauseCommand, 4); bind(c.playCommand, 5);
            [c.changePlaybackPositionCommand addTargetWithHandler:^MPRemoteCommandHandlerStatus(MPRemoteCommandEvent *event) {
                NSTimeInterval t = ((MPChangePlaybackPositionCommandEvent *)event).positionTime;
                if (!isfinite(t) || t < 0 || t > UINT32_MAX) return MPRemoteCommandHandlerStatusCommandFailed;
                tein_command(-(int64_t)(t * 1000) - 1);
                return MPRemoteCommandHandlerStatusSuccess;
            }];
        });
    }];
}
void tein_ios_media(const char *rawTitle, const char *rawArtist, bool playing, int64_t pos, int64_t dur) {
    NSString *title = [NSString stringWithUTF8String:rawTitle], *artist = [NSString stringWithUTF8String:rawArtist];
    dispatch_async(dispatch_get_main_queue(), ^{
        MPNowPlayingInfoCenter.defaultCenter.nowPlayingInfo = title.length ? @{
            MPMediaItemPropertyTitle: title, MPMediaItemPropertyArtist: artist,
            MPMediaItemPropertyPlaybackDuration: @(dur / 1000.0),
            MPNowPlayingInfoPropertyElapsedPlaybackTime: @(pos / 1000.0),
            MPNowPlayingInfoPropertyPlaybackRate: @(playing ? 1.0 : 0.0)
        } : nil;
    });
}
