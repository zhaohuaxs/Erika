#include "include/erika_flutter_plugin_c_api.h"
#include "include/erika_c_abi.h"

#include <flutter/method_channel.h>
#include <flutter/event_channel.h>
#include <flutter/plugin_registrar_windows.h>
#include <flutter/standard_method_codec.h>


#include <windows.h>
#include <chrono>
#include <cmath>
#include <map>
#include <memory>
#include <mutex>
#include <sstream>
#include <string>
#include <vector>

namespace {

static const char* kChannelName = "erika_flutter/player";
static const char* kEventsChannelName = "erika_flutter/events";
static const int64_t kWindowOverlayViewId = -1;
static const double kDefaultDisplayFps = 60.0;

class ErikaNativeLibrary {
 public:
  static ErikaNativeLibrary* Instance() {
    static ErikaNativeLibrary instance;
    return &instance;
  }

  bool loaded() const { return library_handle_ != nullptr; }
  std::string error_message() const { return error_message_; }

  ErikaPresenterCreateFn create = nullptr;
  ErikaPresenterCreateWithOutputModeFn create_with_output_mode = nullptr;
  ErikaPresenterDestroyFn destroy = nullptr;
  ErikaPresenterOpenFn open = nullptr;
  ErikaPresenterCommandFn play = nullptr;
  ErikaPresenterCommandFn pause = nullptr;
  ErikaPresenterCommandFn stop = nullptr;
  ErikaPresenterCommandFn close = nullptr;
  ErikaPresenterSeekFn seek = nullptr;
  ErikaPresenterSetPlaybackRateFn set_playback_rate = nullptr;
  ErikaPresenterSetVolumeFn set_volume = nullptr;
  ErikaPresenterSelectTrackFn select_audio_track = nullptr;
  ErikaPresenterSelectTrackFn select_subtitle_track = nullptr;
  ErikaPresenterAddExternalSubtitleFn add_external_subtitle = nullptr;
  ErikaPresenterRemoveSubtitleTrackFn remove_subtitle_track = nullptr;
  ErikaPresenterTrackSelectionFn track_selection = nullptr;
  ErikaPresenterTracksFn tracks = nullptr;
  ErikaTrackInfoFreeFn free_track_info = nullptr;
  ErikaDanmakuTrackInfoFreeFn free_danmaku_track_info = nullptr;
  ErikaPresenterLoadDanmakuFn load_danmaku_file = nullptr;
  ErikaPresenterLoadDanmakuFn load_danmaku_json = nullptr;
  ErikaPresenterAddDanmakuTrackFn add_danmaku_track_file = nullptr;
  ErikaPresenterAddDanmakuTrackFn add_danmaku_track_json = nullptr;
  ErikaPresenterRemoveDanmakuTrackFn remove_danmaku_track = nullptr;
  ErikaPresenterSetDanmakuTrackEnabledFn set_danmaku_track_enabled = nullptr;
  ErikaPresenterSetDanmakuTrackOffsetFn set_danmaku_track_offset = nullptr;
  ErikaPresenterSetDanmakuGlobalOffsetFn set_danmaku_global_offset = nullptr;
  ErikaPresenterDanmakuTracksFn danmaku_tracks = nullptr;
  ErikaPresenterClearDanmakuFn clear_danmaku = nullptr;
  ErikaPresenterSetDanmakuEnabledFn set_danmaku_enabled = nullptr;
  ErikaPresenterSetDanmakuConfigPtrFn set_danmaku_config_ptr = nullptr;
  ErikaPresenterGetDanmakuConfigFn get_danmaku_config = nullptr;
  ErikaPresenterSetDanmakuFontFn set_danmaku_font = nullptr;
  ErikaPresenterSetDanmakuBlockWordsJsonFn set_danmaku_block_words_json = nullptr;
  ErikaPresenterAttachWgpuSurfaceFn attach_wgpu_surface = nullptr;
  ErikaPresenterResizeSurfaceFn resize_surface = nullptr;
  ErikaPresenterDetachSurfaceFn detach_surface = nullptr;
  ErikaPresenterRenderTickFn render_tick = nullptr;
  ErikaPresenterPollEventFn poll_event = nullptr;

 private:
  ErikaNativeLibrary() {
    std::vector<std::string> candidates;

    char env_path[MAX_PATH] = {};
    if (GetEnvironmentVariableA("ERIKA_CAPI_DLL", env_path, MAX_PATH) > 0) {
      candidates.push_back(env_path);
    }

    char exe_path[MAX_PATH] = {};
    if (GetModuleFileNameA(nullptr, exe_path, MAX_PATH) > 0) {
      std::string exe_dir(exe_path);
      auto last_sep = exe_dir.find_last_of("\\/");
      if (last_sep != std::string::npos) {
        exe_dir = exe_dir.substr(0, last_sep + 1);
        candidates.push_back(exe_dir + "liberika_capi.dll");
      }
    }

    candidates.push_back("liberika_capi.dll");

    for (const auto& path : candidates) {
      HMODULE handle = LoadLibraryA(path.c_str());
      if (handle) {
        library_handle_ = handle;
        LoadSymbols();
        return;
      }
    }

    std::ostringstream oss;
    oss << "Unable to load liberika_capi.dll. Tried: ";
    for (size_t i = 0; i < candidates.size(); ++i) {
      if (i > 0) oss << ", ";
      oss << candidates[i];
    }
    error_message_ = oss.str();
  }

  ~ErikaNativeLibrary() {
    if (library_handle_) {
      FreeLibrary(static_cast<HMODULE>(library_handle_));
    }
  }

  void LoadSymbols() {
    auto load = [this](const char* name) -> void* {
      return reinterpret_cast<void*>(
          GetProcAddress(static_cast<HMODULE>(library_handle_), name));
    };

    create = reinterpret_cast<ErikaPresenterCreateFn>(load("erika_presenter_create"));
    create_with_output_mode = reinterpret_cast<ErikaPresenterCreateWithOutputModeFn>(load("erika_presenter_create_with_output_mode"));
    destroy = reinterpret_cast<ErikaPresenterDestroyFn>(load("erika_presenter_destroy"));
    open = reinterpret_cast<ErikaPresenterOpenFn>(load("erika_presenter_open"));
    play = reinterpret_cast<ErikaPresenterCommandFn>(load("erika_presenter_play"));
    pause = reinterpret_cast<ErikaPresenterCommandFn>(load("erika_presenter_pause"));
    stop = reinterpret_cast<ErikaPresenterCommandFn>(load("erika_presenter_stop"));
    close = reinterpret_cast<ErikaPresenterCommandFn>(load("erika_presenter_close"));
    seek = reinterpret_cast<ErikaPresenterSeekFn>(load("erika_presenter_seek"));
    set_playback_rate = reinterpret_cast<ErikaPresenterSetPlaybackRateFn>(load("erika_presenter_set_playback_rate"));
    set_volume = reinterpret_cast<ErikaPresenterSetVolumeFn>(load("erika_presenter_set_volume"));
    select_audio_track = reinterpret_cast<ErikaPresenterSelectTrackFn>(load("erika_presenter_select_audio_track"));
    select_subtitle_track = reinterpret_cast<ErikaPresenterSelectTrackFn>(load("erika_presenter_select_subtitle_track"));
    add_external_subtitle = reinterpret_cast<ErikaPresenterAddExternalSubtitleFn>(load("erika_presenter_add_external_subtitle"));
    remove_subtitle_track = reinterpret_cast<ErikaPresenterRemoveSubtitleTrackFn>(load("erika_presenter_remove_subtitle_track"));
    track_selection = reinterpret_cast<ErikaPresenterTrackSelectionFn>(load("erika_presenter_track_selection"));
    tracks = reinterpret_cast<ErikaPresenterTracksFn>(load("erika_presenter_tracks"));
    free_track_info = reinterpret_cast<ErikaTrackInfoFreeFn>(load("erika_track_info_free"));
    free_danmaku_track_info = reinterpret_cast<ErikaDanmakuTrackInfoFreeFn>(load("erika_danmaku_track_info_free"));
    load_danmaku_file = reinterpret_cast<ErikaPresenterLoadDanmakuFn>(load("erika_presenter_load_danmaku_file"));
    load_danmaku_json = reinterpret_cast<ErikaPresenterLoadDanmakuFn>(load("erika_presenter_load_danmaku_json"));
    add_danmaku_track_file = reinterpret_cast<ErikaPresenterAddDanmakuTrackFn>(load("erika_presenter_add_danmaku_track_file"));
    add_danmaku_track_json = reinterpret_cast<ErikaPresenterAddDanmakuTrackFn>(load("erika_presenter_add_danmaku_track_json"));
    remove_danmaku_track = reinterpret_cast<ErikaPresenterRemoveDanmakuTrackFn>(load("erika_presenter_remove_danmaku_track"));
    set_danmaku_track_enabled = reinterpret_cast<ErikaPresenterSetDanmakuTrackEnabledFn>(load("erika_presenter_set_danmaku_track_enabled"));
    set_danmaku_track_offset = reinterpret_cast<ErikaPresenterSetDanmakuTrackOffsetFn>(load("erika_presenter_set_danmaku_track_offset"));
    set_danmaku_global_offset = reinterpret_cast<ErikaPresenterSetDanmakuGlobalOffsetFn>(load("erika_presenter_set_danmaku_global_offset"));
    danmaku_tracks = reinterpret_cast<ErikaPresenterDanmakuTracksFn>(load("erika_presenter_danmaku_tracks"));
    clear_danmaku = reinterpret_cast<ErikaPresenterClearDanmakuFn>(load("erika_presenter_clear_danmaku"));
    set_danmaku_enabled = reinterpret_cast<ErikaPresenterSetDanmakuEnabledFn>(load("erika_presenter_set_danmaku_enabled"));
    set_danmaku_config_ptr = reinterpret_cast<ErikaPresenterSetDanmakuConfigPtrFn>(load("erika_presenter_set_danmaku_config_ptr"));
    get_danmaku_config = reinterpret_cast<ErikaPresenterGetDanmakuConfigFn>(load("erika_presenter_get_danmaku_config"));
    set_danmaku_font = reinterpret_cast<ErikaPresenterSetDanmakuFontFn>(load("erika_presenter_set_danmaku_font"));
    set_danmaku_block_words_json = reinterpret_cast<ErikaPresenterSetDanmakuBlockWordsJsonFn>(load("erika_presenter_set_danmaku_block_words_json"));
    attach_wgpu_surface = reinterpret_cast<ErikaPresenterAttachWgpuSurfaceFn>(load("erika_presenter_attach_wgpu_surface"));
    resize_surface = reinterpret_cast<ErikaPresenterResizeSurfaceFn>(load("erika_presenter_resize_surface"));
    detach_surface = reinterpret_cast<ErikaPresenterDetachSurfaceFn>(load("erika_presenter_detach_surface"));
    render_tick = reinterpret_cast<ErikaPresenterRenderTickFn>(load("erika_presenter_render_tick"));
    poll_event = reinterpret_cast<ErikaPresenterPollEventFn>(load("erika_presenter_poll_event"));
  }

  void* library_handle_ = nullptr;
  std::string error_message_;
};

class ErikaPlayerHost {
 public:
  ErikaPlayerHost(int64_t id, ErikaNativeLibrary* lib, void* handle)
      : id_(id), lib_(lib), handle_(handle) {}

  ~ErikaPlayerHost() {
    if (lib_ && lib_->detach_surface && handle_) {
      lib_->detach_surface(handle_);
    }
    if (lib_ && lib_->destroy && handle_) {
      lib_->destroy(handle_);
    }
  }

  int64_t id() const { return id_; }
  void* handle() const { return handle_; }
  ErikaNativeLibrary* lib() const { return lib_; }

  void set_attached_texture_id(int64_t tid) { attached_texture_id_ = tid; }
  int64_t attached_texture_id() const { return attached_texture_id_; }

  void set_attached_hwnd(HWND hwnd) { attached_hwnd_ = hwnd; }
  HWND attached_hwnd() const { return attached_hwnd_; }

  void set_start_time(std::chrono::steady_clock::time_point t) { start_time_ = t; }
  std::chrono::steady_clock::time_point start_time() const { return start_time_; }

  ErikaDanmakuConfig& danmaku_config() { return danmaku_config_; }
  const ErikaDanmakuConfig& danmaku_config() const { return danmaku_config_; }

  void RefreshDanmakuConfigSnapshot() {
    if (!lib_->get_danmaku_config || !handle_) return;
    ErikaDanmakuConfig config = {};
    if (lib_->get_danmaku_config(handle_, &config) == 0) {
      danmaku_config_ = config;
    }
  }

 private:
  int64_t id_;
  ErikaNativeLibrary* lib_;
  void* handle_;
  int64_t attached_texture_id_ = -1;
  HWND attached_hwnd_ = nullptr;
  std::chrono::steady_clock::time_point start_time_ = std::chrono::steady_clock::now();
  ErikaDanmakuConfig danmaku_config_ = {};
};


flutter::EncodableValue TrackInfoToMap(const ErikaTrackInfo& info) {
  return flutter::EncodableValue(flutter::EncodableMap{
      {"id", static_cast<int64_t>(info.id)},
      {"kind", static_cast<int32_t>(info.kind)},
      {"source", static_cast<int32_t>(info.source)},
      {"selected", info.selected != 0},
      {"canRemove", info.can_remove != 0},
      {"title", info.title ? flutter::EncodableValue(std::string(info.title)) : flutter::EncodableValue()},
      {"language", info.language ? flutter::EncodableValue(std::string(info.language)) : flutter::EncodableValue()},
      {"codec", info.codec ? flutter::EncodableValue(std::string(info.codec)) : flutter::EncodableValue()},
  });
}

flutter::EncodableValue DanmakuTrackInfoToMap(const ErikaDanmakuTrackInfo& info) {
  return flutter::EncodableValue(flutter::EncodableMap{
      {"id", static_cast<int64_t>(info.id)},
      {"enabled", info.enabled != 0},
      {"offsetMicros", static_cast<int64_t>(info.offset_micros)},
      {"itemCount", info.item_count},
      {"name", info.name ? flutter::EncodableValue(std::string(info.name)) : flutter::EncodableValue()},
      {"source", info.source ? flutter::EncodableValue(std::string(info.source)) : flutter::EncodableValue()},
  });
}

flutter::EncodableValue EventToMap(const ErikaEvent& event, int64_t player_id) {
  return flutter::EncodableValue(flutter::EncodableMap{
      {"playerId", player_id},
      {"kind", static_cast<int32_t>(event.kind)},
      {"status", static_cast<int32_t>(event.status)},
      {"state", static_cast<int32_t>(event.state)},
      {"durationMicros", static_cast<int64_t>(event.duration_micros)},
      {"positionMicros", static_cast<int64_t>(event.position_micros)},
      {"buffering", event.buffering != 0},
      {"video", flutter::EncodableValue(flutter::EncodableMap{
                    {"width", static_cast<int32_t>(event.video.width)},
                    {"height", static_cast<int32_t>(event.video.height)},
                    {"primaries", static_cast<int32_t>(event.video.primaries)},
                    {"transfer", static_cast<int32_t>(event.video.transfer)},
                })},
      {"tracks", flutter::EncodableValue(flutter::EncodableMap{
                    {"video", static_cast<int32_t>(event.tracks.video)},
                    {"audio", static_cast<int32_t>(event.tracks.audio)},
                    {"subtitle", static_cast<int32_t>(event.tracks.subtitle)},
                })},
  });
}

int64_t GetInt64(const flutter::EncodableMap& map, const std::string& key, int64_t default_val = 0) {
  auto it = map.find(key);
  if (it == map.end()) return default_val;
  if (auto* v = std::get_if<int>(&it->second)) return static_cast<int64_t>(*v);
  if (auto* v = std::get_if<int64_t>(&it->second)) return *v;
  if (auto* v = std::get_if<double>(&it->second)) return static_cast<int64_t>(*v);
  return default_val;
}

double GetDouble(const flutter::EncodableMap& map, const std::string& key, double default_val = 0.0) {
  auto it = map.find(key);
  if (it == map.end()) return default_val;
  if (auto* v = std::get_if<double>(&it->second)) return *v;
  if (auto* v = std::get_if<int>(&it->second)) return static_cast<double>(*v);
  if (auto* v = std::get_if<int64_t>(&it->second)) return static_cast<double>(*v);
  return default_val;
}

std::string GetString(const flutter::EncodableMap& map, const std::string& key, const std::string& default_val = "") {
  auto it = map.find(key);
  if (it == map.end()) return default_val;
  if (auto* v = std::get_if<std::string>(&it->second)) return *v;
  return default_val;
}

bool GetBool(const flutter::EncodableMap& map, const std::string& key, bool default_val = false) {
  auto it = map.find(key);
  if (it == map.end()) return default_val;
  if (auto* v = std::get_if<bool>(&it->second)) return *v;
  if (auto* v = std::get_if<int>(&it->second)) return *v != 0;
  return default_val;
}

ErikaStatus CheckStatus(ErikaStatus status, const std::string& operation) {
  return status;
}

ErikaDanmakuConfig BuildDanmakuConfig(const flutter::EncodableMap& args, const ErikaDanmakuConfig& base) {
  ErikaDanmakuConfig config = base;
  if (args.count("enabled")) config.enabled = GetBool(args, "enabled") ? 1 : 0;
  if (args.count("fontSize")) config.font_size = static_cast<float>(GetDouble(args, "fontSize"));
  if (args.count("opacity")) config.opacity = static_cast<float>(GetDouble(args, "opacity"));
  if (args.count("displayArea")) config.display_area = static_cast<float>(GetDouble(args, "displayArea"));
  if (args.count("scrollDurationSeconds")) config.scroll_duration_seconds = static_cast<float>(GetDouble(args, "scrollDurationSeconds"));
  if (args.count("scrollSpeedFactor")) config.scroll_speed_factor = static_cast<float>(GetDouble(args, "scrollSpeedFactor"));
  if (args.count("trackGapRatio")) config.track_gap_ratio = static_cast<float>(GetDouble(args, "trackGapRatio"));
  if (args.count("outlineWidth")) config.outline_width = static_cast<float>(GetDouble(args, "outlineWidth"));
  if (args.count("shadowOffsetX")) config.shadow_offset_x = static_cast<float>(GetDouble(args, "shadowOffsetX"));
  if (args.count("shadowOffsetY")) config.shadow_offset_y = static_cast<float>(GetDouble(args, "shadowOffsetY"));
  if (args.count("mergeDuplicates")) config.merge_duplicates = GetBool(args, "mergeDuplicates") ? 1 : 0;
  if (args.count("allowStacking")) config.allow_stacking = GetBool(args, "allowStacking") ? 1 : 0;
  if (args.count("allowScrollOverwrite")) config.allow_scroll_overwrite = GetBool(args, "allowScrollOverwrite") ? 1 : 0;
  if (args.count("maxQuantity")) { auto v = GetInt64(args, "maxQuantity"); if (v > 0) config.max_quantity = static_cast<uint32_t>(v); }
  if (args.count("maxLinesPerMode")) { auto v = GetInt64(args, "maxLinesPerMode"); if (v > 0) config.max_lines_per_mode = static_cast<uint32_t>(v); }
  if (args.count("blockTop")) config.block_top = GetBool(args, "blockTop") ? 1 : 0;
  if (args.count("blockBottom")) config.block_bottom = GetBool(args, "blockBottom") ? 1 : 0;
  if (args.count("blockScroll")) config.block_scroll = GetBool(args, "blockScroll") ? 1 : 0;
  if (args.count("shadowStyle")) config.shadow_style = static_cast<int32_t>(GetInt64(args, "shadowStyle"));
  return config;
}

static const char* kVideoViewType = "erika_flutter/video_view";


class ErikaFlutterPlugin : public flutter::Plugin {
 public:
  static void RegisterWithRegistrar(flutter::PluginRegistrarWindows* registrar);

  ErikaFlutterPlugin(flutter::PluginRegistrarWindows* registrar);
  virtual ~ErikaFlutterPlugin();

 private:
  void HandleMethodCall(
      const flutter::MethodCall<flutter::EncodableValue>& method_call,
      std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>> result);

  void OnEventListen(std::unique_ptr<flutter::EventResult<flutter::EncodableValue>> result);
  void OnEventCancel();

  ErikaPlayerHost* GetPlayerHost(const flutter::EncodableMap& args);
  ErikaPlayerHost* CreatePlayer(const flutter::EncodableValue* arguments);
  void DestroyPlayer(int64_t player_id);

  void RenderTickForPlayer(ErikaPlayerHost* host);
  void PollEventsForPlayer(ErikaPlayerHost* host);
  void StartRenderTimer();
  void StopRenderTimer();

  flutter::PluginRegistrarWindows* registrar_;
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> channel_;
  std::unique_ptr<flutter::EventChannel<flutter::EncodableValue>> event_channel_;
  std::unique_ptr<flutter::StreamHandler<flutter::EncodableValue>> stream_handler_;

  std::map<int64_t, std::unique_ptr<ErikaPlayerHost>> players_;
  int64_t next_player_id_ = 1;

  std::unique_ptr<flutter::EventResult<flutter::EncodableValue>> event_sink_;
  UINT_PTR render_timer_id_ = 0;
  static void CALLBACK RenderTimerProc(HWND hwnd, UINT msg, UINT_PTR id, DWORD elapsed_ms);
  static ErikaFlutterPlugin* timer_plugin_instance_;
};

ErikaFlutterPlugin* ErikaFlutterPlugin::timer_plugin_instance_ = nullptr;

void ErikaFlutterPlugin::RegisterWithRegistrar(flutter::PluginRegistrarWindows* registrar) {
  auto plugin = std::make_unique<ErikaFlutterPlugin>(registrar);
  registrar->AddPlugin(std::move(plugin));

}

ErikaFlutterPlugin::ErikaFlutterPlugin(flutter::PluginRegistrarWindows* registrar)
    : registrar_(registrar) {
  channel_ = std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
      registrar->messenger(), kChannelName,
      &flutter::StandardMethodCodec::GetInstance());
  auto channel_ptr = channel_.get();
  channel_->SetMethodCallHandler(
      [this](const auto& call, auto result) {
        HandleMethodCall(call, std::move(result));
      });

  event_channel_ = std::make_unique<flutter::EventChannel<flutter::EncodableValue>>(
      registrar->messenger(), kEventsChannelName,
      &flutter::StandardMethodCodec::GetInstance());
  event_channel_->SetStreamHandler(
      std::make_unique<flutter::StreamHandlerFunctions<flutter::EncodableValue>>(
          [this](const flutter::EncodableValue* args,
                 std::unique_ptr<flutter::EventResult<flutter::EncodableValue>> result) {
            OnEventListen(std::move(result));
            return nullptr;
          },
          [this](const flutter::EncodableValue* args) {
            OnEventCancel();
            return nullptr;
          }));
}

ErikaFlutterPlugin::~ErikaFlutterPlugin() {
  StopRenderTimer();
  players_.clear();
}

void ErikaFlutterPlugin::OnEventListen(std::unique_ptr<flutter::EventResult<flutter::EncodableValue>> result) {
  event_sink_ = std::move(result);
  StartRenderTimer();
}

void ErikaFlutterPlugin::OnEventCancel() {
  event_sink_.reset();
  StopRenderTimer();
}

ErikaPlayerHost* ErikaFlutterPlugin::GetPlayerHost(const flutter::EncodableMap& args) {
  int64_t player_id = GetInt64(args, "playerId", -1);
  auto it = players_.find(player_id);
  if (it == players_.end()) return nullptr;
  return it->second.get();
}

ErikaPlayerHost* ErikaFlutterPlugin::CreatePlayer(const flutter::EncodableValue* arguments) {
  auto* lib = ErikaNativeLibrary::Instance();
  if (!lib->loaded() || !lib->create) return nullptr;

  int32_t output_mode = 0;
  float edr_headroom = 1.0f;
  if (arguments && std::holds_alternative<flutter::EncodableMap>(*arguments)) {
    const auto& args = std::get<flutter::EncodableMap>(*arguments);
    if (args.count("outputMode")) {
      output_mode = static_cast<int32_t>(GetInt64(args, "outputMode"));
    }
    if (args.count("edrHeadroom")) {
      edr_headroom = static_cast<float>(GetDouble(args, "edrHeadroom"));
    }
  }

  void* handle = nullptr;
  if (lib->create_with_output_mode) {
    handle = lib->create_with_output_mode(output_mode, edr_headroom);
  } else {
    handle = lib->create();
  }
  if (!handle) return nullptr;

  int64_t id = next_player_id_++;
  auto host = std::make_unique<ErikaPlayerHost>(id, lib, handle);
  host->set_start_time(std::chrono::steady_clock::now());
  host->RefreshDanmakuConfigSnapshot();
  auto* raw = host.get();
  players_[id] = std::move(host);
  StartRenderTimer();
  return raw;
}

void ErikaFlutterPlugin::DestroyPlayer(int64_t player_id) {
  players_.erase(player_id);
  if (players_.empty()) {
    StopRenderTimer();
  }
}

void CALLBACK ErikaFlutterPlugin::RenderTimerProc(HWND hwnd, UINT msg, UINT_PTR id, DWORD elapsed_ms) {
  if (!timer_plugin_instance_) return;
  auto* plugin = timer_plugin_instance_;
  for (auto& [pid, host] : plugin->players_) {
    plugin->RenderTickForPlayer(host.get());
    plugin->PollEventsForPlayer(host.get());
  }
}

void ErikaFlutterPlugin::StartRenderTimer() {
  if (render_timer_id_ != 0) return;
  timer_plugin_instance_ = this;
  render_timer_id_ = SetTimer(nullptr, 0, 16, RenderTimerProc);
}

void ErikaFlutterPlugin::StopRenderTimer() {
  if (render_timer_id_ != 0) {
    KillTimer(nullptr, render_timer_id_);
    render_timer_id_ = 0;
  }
  if (timer_plugin_instance_ == this) {
    timer_plugin_instance_ = nullptr;
  }
}

void ErikaFlutterPlugin::RenderTickForPlayer(ErikaPlayerHost* host) {
  if (!host || !host->lib()->render_tick || !host->handle()) return;
  auto now = std::chrono::steady_clock::now();
  double time_seconds = std::chrono::duration<double>(now - host->start_time()).count();
  ErikaPresenterStats stats = {};
  host->lib()->render_tick(host->handle(), time_seconds, &stats);
}

void ErikaFlutterPlugin::PollEventsForPlayer(ErikaPlayerHost* host) {
  if (!host || !host->lib()->poll_event || !host->handle()) return;
  if (!event_sink_) return;
  while (true) {
    ErikaEvent event = {};
    ErikaStatus status = host->lib()->poll_event(host->handle(), &event);
    if (status == 0) {
      auto value = EventToMap(event, host->id());
      event_sink_->Success(value);
      continue;
    }
    break;
  }
}

void ErikaFlutterPlugin::HandleMethodCall(
    const flutter::MethodCall<flutter::EncodableValue>& method_call,
    std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>> result) {

  const auto& method = method_call.method_name();
  const auto* arguments = &method_call.arguments();

  auto get_args = [&]() -> flutter::EncodableMap {
    if (arguments && std::holds_alternative<flutter::EncodableMap>(*arguments)) {
      return std::get<flutter::EncodableMap>(*arguments);
    }
    return {};
  };

  auto error = [&result](const std::string& msg) {
    result->Error("ERIKA_ERROR", msg, flutter::EncodableValue());
  };

  auto* lib = ErikaNativeLibrary::Instance();

  if (method == "create") {
    auto* host = CreatePlayer(arguments);
    if (!host) {
      if (!lib->loaded()) {
        error(lib->error_message());
      } else {
        error("erika_presenter_create returned null.");
      }
      return;
    }
    result->Success(flutter::EncodableValue(host->id()));
    return;
  }

  if (method == "dispose") {
    auto args = get_args();
    int64_t player_id = GetInt64(args, "playerId", -1);
    DestroyPlayer(player_id);
    result->Success(flutter::EncodableValue());
    return;
  }

  auto args = get_args();
  auto* host = GetPlayerHost(args);
  if (!host) {
    error("Player not found.");
    return;
  }
  auto* handle = host->handle();

  if (method == "open") {
    std::string uri = GetString(args, "uri");
    if (uri.empty()) { error("uri is required."); return; }
    ErikaStatus status = lib->open(handle, uri.c_str());
    if (status != 0) { error("open failed with status " + std::to_string(status)); return; }
    result->Success(flutter::EncodableValue());
  } else if (method == "play") {
    ErikaStatus status = lib->play(handle);
    if (status != 0) { error("play failed"); return; }
    result->Success(flutter::EncodableValue());
  } else if (method == "pause") {
    ErikaStatus status = lib->pause(handle);
    if (status != 0) { error("pause failed"); return; }
    result->Success(flutter::EncodableValue());
  } else if (method == "stop") {
    ErikaStatus status = lib->stop(handle);
    if (status != 0) { error("stop failed"); return; }
    result->Success(flutter::EncodableValue());
  } else if (method == "close") {
    ErikaStatus status = lib->close(handle);
    if (status != 0) { error("close failed"); return; }
    result->Success(flutter::EncodableValue());
  } else if (method == "seek") {
    uint64_t pos = static_cast<uint64_t>(GetInt64(args, "positionMicros"));
    ErikaStatus status = lib->seek(handle, pos);
    if (status != 0) { error("seek failed"); return; }
    result->Success(flutter::EncodableValue());
  } else if (method == "setPlaybackRate") {
    if (!lib->set_playback_rate) { error("set_playback_rate not available"); return; }
    double rate = GetDouble(args, "rate");
    ErikaStatus status = lib->set_playback_rate(handle, rate);
    if (status != 0) { error("set_playback_rate failed"); return; }
    result->Success(flutter::EncodableValue());
  } else if (method == "setVolume") {
    if (!lib->set_volume) { error("set_volume not available"); return; }
    double volume = GetDouble(args, "volume");
    volume = std::isfinite(volume) ? std::clamp(volume, 0.0, 1.0) : 1.0;
    ErikaStatus status = lib->set_volume(handle, volume);
    if (status != 0) { error("set_volume failed"); return; }
    result->Success(flutter::EncodableValue());
  } else if (method == "addExternalSubtitle") {
    std::string uri = GetString(args, "uri");
    if (uri.empty()) { error("uri is required."); return; }
    int64_t track_id = 0;
    ErikaStatus status = lib->add_external_subtitle(handle, uri.c_str(), &track_id);
    if (status != 0) { error("add_external_subtitle failed"); return; }
    result->Success(flutter::EncodableValue(track_id));
  } else if (method == "removeSubtitleTrack") {
    int64_t track_id = GetInt64(args, "trackId");
    ErikaStatus status = lib->remove_subtitle_track(handle, track_id);
    if (status != 0) { error("remove_subtitle_track failed"); return; }
    result->Success(flutter::EncodableValue());
  } else if (method == "selectAudioTrack") {
    int64_t track_id = GetInt64(args, "trackId", -1);
    ErikaStatus status = lib->select_audio_track(handle, track_id);
    if (status != 0) { error("select_audio_track failed"); return; }
    result->Success(flutter::EncodableValue());
  } else if (method == "selectSubtitleTrack") {
    int64_t track_id = GetInt64(args, "trackId", -1);
    ErikaStatus status = lib->select_subtitle_track(handle, track_id);
    if (status != 0) { error("select_subtitle_track failed"); return; }
    result->Success(flutter::EncodableValue());
  } else if (method == "tracks") {
    size_t count = 0;
    lib->tracks(handle, nullptr, 0, &count);
    if (count == 0) {
      result->Success(flutter::EncodableValue(flutter::EncodableList()));
      return;
    }
    std::vector<ErikaTrackInfo> track_buf(count);
    size_t written = 0;
    lib->tracks(handle, track_buf.data(), track_buf.size(), &written);
    flutter::EncodableList list;
    for (size_t i = 0; i < std::min(written, count); ++i) {
      list.push_back(TrackInfoToMap(track_buf[i]));
    }
    if (lib->free_track_info) {
      for (auto& t : track_buf) {
        lib->free_track_info(&t);
      }
    }
    result->Success(flutter::EncodableValue(list));
  } else if (method == "loadDanmakuFile") {
    if (!lib->load_danmaku_file) { error("load_danmaku_file not available"); return; }
    std::string uri = GetString(args, "uri");
    if (uri.empty()) { error("uri is required."); return; }
    ErikaStatus status = lib->load_danmaku_file(handle, uri.c_str());
    if (status != 0) { error("load_danmaku_file failed"); return; }
    result->Success(flutter::EncodableValue());
  } else if (method == "loadDanmakuJson") {
    if (!lib->load_danmaku_json) { error("load_danmaku_json not available"); return; }
    std::string json = GetString(args, "json");
    if (json.empty()) { error("json is required."); return; }
    ErikaStatus status = lib->load_danmaku_json(handle, json.c_str());
    if (status != 0) { error("load_danmaku_json failed"); return; }
    result->Success(flutter::EncodableValue());
  } else if (method == "addDanmakuTrackFile") {
    if (!lib->add_danmaku_track_file) { error("add_danmaku_track_file not available"); return; }
    std::string uri = GetString(args, "uri");
    std::string name = GetString(args, "name");
    int64_t offset = GetInt64(args, "offsetMicros");
    uint64_t track_id = 0;
    ErikaStatus status = lib->add_danmaku_track_file(handle, uri.c_str(), name.empty() ? nullptr : name.c_str(), offset, &track_id);
    if (status != 0) { error("add_danmaku_track_file failed"); return; }
    result->Success(flutter::EncodableValue(static_cast<int64_t>(track_id)));
  } else if (method == "addDanmakuTrackJson") {
    if (!lib->add_danmaku_track_json) { error("add_danmaku_track_json not available"); return; }
    std::string json = GetString(args, "json");
    std::string name = GetString(args, "name");
    int64_t offset = GetInt64(args, "offsetMicros");
    uint64_t track_id = 0;
    ErikaStatus status = lib->add_danmaku_track_json(handle, json.c_str(), name.empty() ? nullptr : name.c_str(), offset, &track_id);
    if (status != 0) { error("add_danmaku_track_json failed"); return; }
    result->Success(flutter::EncodableValue(static_cast<int64_t>(track_id)));
  } else if (method == "removeDanmakuTrack") {
    if (!lib->remove_danmaku_track) { error("remove_danmaku_track not available"); return; }
    uint64_t track_id = static_cast<uint64_t>(GetInt64(args, "trackId"));
    ErikaStatus status = lib->remove_danmaku_track(handle, track_id);
    if (status != 0) { error("remove_danmaku_track failed"); return; }
    result->Success(flutter::EncodableValue());
  } else if (method == "setDanmakuTrackEnabled") {
    if (!lib->set_danmaku_track_enabled) { error("set_danmaku_track_enabled not available"); return; }
    uint64_t track_id = static_cast<uint64_t>(GetInt64(args, "trackId"));
    bool enabled = GetBool(args, "enabled", true);
    ErikaStatus status = lib->set_danmaku_track_enabled(handle, track_id, enabled);
    if (status != 0) { error("set_danmaku_track_enabled failed"); return; }
    result->Success(flutter::EncodableValue());
  } else if (method == "setDanmakuTrackOffset") {
    if (!lib->set_danmaku_track_offset) { error("set_danmaku_track_offset not available"); return; }
    uint64_t track_id = static_cast<uint64_t>(GetInt64(args, "trackId"));
    int64_t offset = GetInt64(args, "offsetMicros");
    ErikaStatus status = lib->set_danmaku_track_offset(handle, track_id, offset);
    if (status != 0) { error("set_danmaku_track_offset failed"); return; }
    result->Success(flutter::EncodableValue());
  } else if (method == "setDanmakuGlobalOffset") {
    if (!lib->set_danmaku_global_offset) { error("set_danmaku_global_offset not available"); return; }
    int64_t offset = GetInt64(args, "offsetMicros");
    ErikaStatus status = lib->set_danmaku_global_offset(handle, offset);
    if (status != 0) { error("set_danmaku_global_offset failed"); return; }
    result->Success(flutter::EncodableValue());
  } else if (method == "danmakuTracks") {
    if (!lib->danmaku_tracks) { error("danmaku_tracks not available"); return; }
    size_t count = 0;
    lib->danmaku_tracks(handle, nullptr, 0, &count);
    if (count == 0) {
      result->Success(flutter::EncodableValue(flutter::EncodableList()));
      return;
    }
    std::vector<ErikaDanmakuTrackInfo> track_buf(count);
    size_t written = 0;
    lib->danmaku_tracks(handle, track_buf.data(), track_buf.size(), &written);
    flutter::EncodableList list;
    for (size_t i = 0; i < std::min(written, count); ++i) {
      list.push_back(DanmakuTrackInfoToMap(track_buf[i]));
    }
    if (lib->free_danmaku_track_info) {
      for (auto& t : track_buf) {
        lib->free_danmaku_track_info(&t);
      }
    }
    result->Success(flutter::EncodableValue(list));
  } else if (method == "clearDanmaku") {
    if (!lib->clear_danmaku) { error("clear_danmaku not available"); return; }
    ErikaStatus status = lib->clear_danmaku(handle);
    if (status != 0) { error("clear_danmaku failed"); return; }
    result->Success(flutter::EncodableValue());
  } else if (method == "setDanmakuEnabled") {
    if (!lib->set_danmaku_enabled) { error("set_danmaku_enabled not available"); return; }
    bool enabled = GetBool(args, "enabled", true);
    ErikaStatus status = lib->set_danmaku_enabled(handle, enabled);
    if (status != 0) { error("set_danmaku_enabled failed"); return; }
    host->danmaku_config().enabled = enabled ? 1 : 0;
    result->Success(flutter::EncodableValue());
  } else if (method == "setDanmakuConfig") {
    if (!lib->set_danmaku_config_ptr) { error("set_danmaku_config_ptr not available"); return; }
    auto config = BuildDanmakuConfig(args, host->danmaku_config());
    ErikaStatus status = lib->set_danmaku_config_ptr(handle, &config);
    if (status != 0) { error("set_danmaku_config failed"); return; }
    host->danmaku_config() = config;

    if (args.count("customFontFamily") || args.count("customFontFilePath")) {
      if (lib->set_danmaku_font) {
        std::string family = GetString(args, "customFontFamily");
        std::string file_path = GetString(args, "customFontFilePath");
        lib->set_danmaku_font(handle, family.c_str(), file_path.c_str());
      }
    }
    if (args.count("blockWordsJson")) {
      if (lib->set_danmaku_block_words_json) {
        std::string json = GetString(args, "blockWordsJson");
        lib->set_danmaku_block_words_json(handle, json.c_str());
      }
    }
    host->RefreshDanmakuConfigSnapshot();
    result->Success(flutter::EncodableValue());
  } else if (method == "attachView") {
    int64_t view_id = GetInt64(args, "viewId", -1);
    HWND view_hwnd = nullptr;
    if (view_id > 0) {
      view_hwnd = reinterpret_cast<HWND>(static_cast<intptr_t>(view_id));
    }
    if (!view_hwnd) {
      view_hwnd = registrar_->GetViewWindow();
    }
    if (!view_hwnd) {
      error("No window handle available for surface attachment");
      return;
    }

    RECT rect;
    GetClientRect(view_hwnd, &rect);
    uint32_t width = static_cast<uint32_t>(std::max(1L, rect.right - rect.left));
    uint32_t height = static_cast<uint32_t>(std::max(1L, rect.bottom - rect.top));
    double scale = 1.0;
    UINT dpi = GetDpiForWindow(view_hwnd);
    if (dpi > 0) { scale = static_cast<double>(dpi) / 96.0; }

    if (lib->attach_wgpu_surface) {
      ErikaWgpuSurfaceKind kind = 1;
      ErikaStatus status = lib->attach_wgpu_surface(handle, kind, reinterpret_cast<uint64_t>(view_hwnd), 0, width, height, scale);
      if (status != 0) { error("attach_wgpu_surface failed"); return; }
    }
    host->set_attached_hwnd(view_hwnd);
    host->set_start_time(std::chrono::steady_clock::now());
    result->Success(flutter::EncodableValue());
  } else if (method == "detachView") {
    if (lib->detach_surface) {
      lib->detach_surface(handle);
    }
    host->set_attached_hwnd(nullptr);
    host->set_attached_texture_id(-1);
    result->Success(flutter::EncodableValue());
  } else if (method == "attachOverlay") {
    HWND view_hwnd = registrar_->GetViewWindow();
    if (!view_hwnd) {
      error("No window handle available for overlay attachment");
      return;
    }
    RECT rect;
    GetClientRect(view_hwnd, &rect);
    uint32_t width = static_cast<uint32_t>(std::max(1L, rect.right - rect.left));
    uint32_t height = static_cast<uint32_t>(std::max(1L, rect.bottom - rect.top));
    double scale = 1.0;
    UINT dpi = GetDpiForWindow(view_hwnd);
    if (dpi > 0) { scale = static_cast<double>(dpi) / 96.0; }
    if (lib->attach_wgpu_surface) {
      ErikaWgpuSurfaceKind kind = 1;
      ErikaStatus status = lib->attach_wgpu_surface(handle, kind, reinterpret_cast<uint64_t>(view_hwnd), 0, width, height, scale);
      if (status != 0) { error("attach_wgpu_surface (overlay) failed"); return; }
    }
    host->set_attached_hwnd(view_hwnd);
    host->set_start_time(std::chrono::steady_clock::now());
    result->Success(flutter::EncodableValue(kWindowOverlayViewId));
  } else if (method == "detachOverlay") {
    if (lib->detach_surface) {
      lib->detach_surface(handle);
    }
    host->set_attached_hwnd(nullptr);
    result->Success(flutter::EncodableValue());
  } else if (method == "setOverlayFrame") {
    result->Success(flutter::EncodableValue());
  } else if (method == "screenshot") {
    result->Success(flutter::EncodableValue());
  } else {
    result->NotImplemented();
  }
}

}  // namespace

void ErikaFlutterPluginCApiRegisterWithRegistrar(FlutterPluginRegistrar* registrar) {
  auto* registrar_windows = reinterpret_cast<flutter::PluginRegistrarWindows*>(registrar);
  ErikaFlutterPlugin::RegisterWithRegistrar(registrar_windows);
}