// Minimal PE64 fallback stub for vulkan-1.dll placed inside runtime/media-engine/.
//
// Windows DLL Search Order with SetDllDirectoryW("...\\runtime\\media-engine"):
//   1. Application directory (StreamVault\)
//   2. System directory (C:\Windows\System32\vulkan-1.dll) <-- Hardware GPU drivers ALWAYS win first!
//   3. Windows directory
//   4. runtime\media-engine\vulkan-1.dll <-- Only reached on GPU-less Windows Sandbox / Hyper-V VMs
//      where System32\vulkan-1.dll is absent, allowing libmpv-2.dll to load cleanly and render
//      via Direct3D 11 (gpu-context=d3d11 / WARP).
//
// Build command:
//   clang --target=x86_64-pc-windows-msvc -O2 -nostdlib -Wl,/DLL -Wl,/NOENTRY \
//     runtime-src/vulkan_fallback_stub.c -fuse-ld=lld-link -o StreamVault/runtime/media-engine/vulkan-1.dll

typedef int VkResult;
#define VK_ERROR_INITIALIZATION_FAILED (-3)

int __stdcall DllMain(void* hinst, unsigned int reason, void* reserved) {
    (void)hinst; (void)reason; (void)reserved;
    return 1;
}

__declspec(dllexport) void* vkGetInstanceProcAddr(void* instance, const char* name) {
    (void)instance; (void)name;
    return (void*)0;
}
__declspec(dllexport) VkResult vkEnumeratePhysicalDevices(void* instance, unsigned int* count, void* devs) {
    (void)instance; (void)devs;
    if (count) *count = 0;
    return VK_ERROR_INITIALIZATION_FAILED;
}
__declspec(dllexport) VkResult vkCreateDisplayPlaneSurfaceKHR(void* a, const void* b, const void* c, void* d) {
    (void)a; (void)b; (void)c; (void)d;
    return VK_ERROR_INITIALIZATION_FAILED;
}
__declspec(dllexport) VkResult vkGetDisplayModePropertiesKHR(void* a, void* b, unsigned int* c, void* d) {
    (void)a; (void)b; (void)d;
    if (c) *c = 0;
    return VK_ERROR_INITIALIZATION_FAILED;
}
__declspec(dllexport) VkResult vkGetDisplayPlaneSupportedDisplaysKHR(void* a, unsigned int b, unsigned int* c, void* d) {
    (void)a; (void)b; (void)d;
    if (c) *c = 0;
    return VK_ERROR_INITIALIZATION_FAILED;
}
__declspec(dllexport) VkResult vkGetPhysicalDeviceDisplayPlanePropertiesKHR(void* a, unsigned int* b, void* c) {
    (void)a; (void)c;
    if (b) *b = 0;
    return VK_ERROR_INITIALIZATION_FAILED;
}
__declspec(dllexport) VkResult vkGetPhysicalDeviceDisplayPropertiesKHR(void* a, unsigned int* b, void* c) {
    (void)a; (void)c;
    if (b) *b = 0;
    return VK_ERROR_INITIALIZATION_FAILED;
}
__declspec(dllexport) void vkGetPhysicalDeviceProperties(void* a, void* b) {
    (void)a; (void)b;
}
__declspec(dllexport) void vkGetPhysicalDeviceProperties2(void* a, void* b) {
    (void)a; (void)b;
}
__declspec(dllexport) void vkGetPhysicalDeviceQueueFamilyProperties2(void* a, unsigned int* b, void* c) {
    (void)a; (void)c;
    if (b) *b = 0;
}
