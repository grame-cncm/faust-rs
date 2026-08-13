#ifndef FAUST_MEMORY_MANAGER_H
#define FAUST_MEMORY_MANAGER_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define FAUST_MEMORY_MANAGER_ABI_VERSION 1u

/* Keep values append-only and synchronized with ffi_common::FaustMemoryType. */
typedef enum faust_memory_type {
    kMemInt32 = 0,
    kMemInt32Ptr = 1,
    kMemFloat32 = 2,
    kMemFloat32Ptr = 3,
    kMemFloat64 = 4,
    kMemFloat64Ptr = 5,
    kMemQuad = 6,
    kMemQuadPtr = 7,
    kMemFixedPoint = 8,
    kMemFixedPointPtr = 9,
    kMemObject = 10,
    kMemObjectPtr = 11,
    kMemSound = 12,
    kMemSoundPtr = 13,
    kMemInt64 = 14,
    kMemInt64Ptr = 15,
    kMemBool = 16,
    kMemBoolPtr = 17
} faust_memory_type;

/*
 * Versioned faust-rs -mem0 ABI. All callbacks are mandatory in version 1.
 * The owner must keep this table and context alive until class destruction.
 */
typedef struct faust_memory_manager {
    uint32_t abi_version;
    size_t struct_size;
    void* context;
    void (*begin)(void* context, size_t count);
    void (*info)(void* context,
                 const char* name,
                 faust_memory_type type,
                 size_t element_count,
                 size_t size_bytes,
                 size_t alignment,
                 uint64_t reads,
                 uint64_t writes);
    void (*end)(void* context);
    void* (*allocate)(void* context, size_t size_bytes, size_t alignment);
    void (*destroy)(void* context,
                    void* address,
                    size_t size_bytes,
                    size_t alignment);
} faust_memory_manager;

#ifdef __cplusplus
}
#endif

#endif
