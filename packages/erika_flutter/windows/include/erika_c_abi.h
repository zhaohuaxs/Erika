#ifndef ERIKA_C_ABI_H_
#define ERIKA_C_ABI_H_

#include <cstdint>
#include <cstddef>

#if defined(__cplusplus)
extern "C" {
#endif

typedef int32_t ErikaStatus;

typedef struct {
  int64_t video;
  int64_t audio;
  int64_t subtitle;
} ErikaTrackSelection;

typedef struct {
  int64_t id;
  int32_t kind;
  int32_t source;
  uint8_t selected;
  uint8_t can_remove;
  const char* title;
  const char* language;
  const char* codec;
} ErikaTrackInfo;

typedef struct {
  uint32_t width;
  uint32_t height;
  uint32_t primaries;
  uint32_t transfer;
} ErikaVideoParams;

typedef struct {
  uint32_t video;
  uint32_t audio;
  uint32_t subtitle;
} ErikaTrackCounts;

typedef struct {
  int32_t kind;
  int32_t status;
  int32_t state;
  int64_t duration_micros;
  uint64_t position_micros;
  uint8_t buffering;
  ErikaVideoParams video;
  ErikaTrackCounts tracks;
} ErikaEvent;

typedef struct {
  uint64_t decoded_video_frames;
  uint64_t rendered_video_frames;
  uint64_t rendered_test_frames;
  uint64_t pushed_audio_frames;
  uint64_t overlay_frames;
  uint64_t danmaku_frames;
  uint64_t danmaku_items;
  uint64_t import_failures;
  uint64_t render_failures;
  uint64_t audio_failures;
} ErikaPresenterStats;

typedef struct {
  uint8_t enabled;
  float font_size;
  float opacity;
  float display_area;
  float scroll_duration_seconds;
  float scroll_speed_factor;
  float track_gap_ratio;
  float outline_width;
  float shadow_offset_x;
  float shadow_offset_y;
  uint8_t merge_duplicates;
  uint8_t allow_stacking;
  uint8_t allow_scroll_overwrite;
  uint32_t max_quantity;
  uint32_t max_lines_per_mode;
  uint8_t block_top;
  uint8_t block_bottom;
  uint8_t block_scroll;
  int32_t shadow_style;
} ErikaDanmakuConfig;

typedef struct {
  uint64_t id;
  uint8_t enabled;
  int64_t offset_micros;
  int item_count;
  const char* name;
  const char* source;
} ErikaDanmakuTrackInfo;

typedef void* ErikaPresenterHandle;

typedef int32_t ErikaWgpuSurfaceKind;

typedef struct {
  int32_t output_mode;
  float edr_headroom;
} ErikaPresenterConfig;

typedef void* (*ErikaPresenterCreateFn)();
typedef void* (*ErikaPresenterCreateWithOutputModeFn)(int32_t, float);
typedef void (*ErikaPresenterDestroyFn)(void*);
typedef ErikaStatus (*ErikaPresenterOpenFn)(void*, const char*);
typedef ErikaStatus (*ErikaPresenterCommandFn)(void*);
typedef ErikaStatus (*ErikaPresenterSeekFn)(void*, uint64_t);
typedef ErikaStatus (*ErikaPresenterSetPlaybackRateFn)(void*, double);
typedef ErikaStatus (*ErikaPresenterSetVolumeFn)(void*, double);
typedef ErikaStatus (*ErikaPresenterSelectTrackFn)(void*, int64_t);
typedef ErikaStatus (*ErikaPresenterAddExternalSubtitleFn)(void*, const char*, int64_t*);
typedef ErikaStatus (*ErikaPresenterRemoveSubtitleTrackFn)(void*, int64_t);
typedef ErikaStatus (*ErikaPresenterTrackSelectionFn)(void*, ErikaTrackSelection*);
typedef ErikaStatus (*ErikaPresenterTracksFn)(void*, ErikaTrackInfo*, size_t, size_t*);
typedef void (*ErikaTrackInfoFreeFn)(ErikaTrackInfo*);
typedef void (*ErikaDanmakuTrackInfoFreeFn)(ErikaDanmakuTrackInfo*);
typedef ErikaStatus (*ErikaPresenterLoadDanmakuFn)(void*, const char*);
typedef ErikaStatus (*ErikaPresenterAddDanmakuTrackFn)(void*, const char*, const char*, int64_t, uint64_t*);
typedef ErikaStatus (*ErikaPresenterRemoveDanmakuTrackFn)(void*, uint64_t);
typedef ErikaStatus (*ErikaPresenterSetDanmakuTrackEnabledFn)(void*, uint64_t, bool);
typedef ErikaStatus (*ErikaPresenterSetDanmakuTrackOffsetFn)(void*, uint64_t, int64_t);
typedef ErikaStatus (*ErikaPresenterSetDanmakuGlobalOffsetFn)(void*, int64_t);
typedef ErikaStatus (*ErikaPresenterDanmakuTracksFn)(void*, ErikaDanmakuTrackInfo*, size_t, size_t*);
typedef ErikaStatus (*ErikaPresenterClearDanmakuFn)(void*);
typedef ErikaStatus (*ErikaPresenterSetDanmakuEnabledFn)(void*, bool);
typedef ErikaStatus (*ErikaPresenterSetDanmakuConfigPtrFn)(void*, const ErikaDanmakuConfig*);
typedef ErikaStatus (*ErikaPresenterGetDanmakuConfigFn)(void*, ErikaDanmakuConfig*);
typedef ErikaStatus (*ErikaPresenterSetDanmakuFontFn)(void*, const char*, const char*);
typedef ErikaStatus (*ErikaPresenterSetDanmakuBlockWordsJsonFn)(void*, const char*);
typedef ErikaStatus (*ErikaPresenterAttachWgpuSurfaceFn)(void*, ErikaWgpuSurfaceKind, uint64_t, uint64_t, uint32_t, uint32_t, double);
typedef ErikaStatus (*ErikaPresenterResizeSurfaceFn)(void*, uint32_t, uint32_t, double);
typedef ErikaStatus (*ErikaPresenterDetachSurfaceFn)(void*);
typedef ErikaStatus (*ErikaPresenterRenderTickFn)(void*, double, ErikaPresenterStats*);
typedef ErikaStatus (*ErikaPresenterPollEventFn)(void*, ErikaEvent*);

#if defined(__cplusplus)
}
#endif

#endif