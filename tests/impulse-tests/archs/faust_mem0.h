/* Self-contained audit manager for the faust-rs mem0 impulse lane. */
#ifndef FAUST_RS_MEM0_AUDIT_H
#define FAUST_RS_MEM0_AUDIT_H

#include <cstddef>
#include <cstdint>
#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <string>
#include <vector>

struct dsp_memory_manager {
    enum MemType {
        kInt32,
        kInt32_ptr,
        kFloat,
        kFloat_ptr,
        kDouble,
        kDouble_ptr,
        kQuad,
        kQuad_ptr,
        kFixedPoint,
        kFixedPoint_ptr,
        kObj,
        kObj_ptr,
        kSound,
        kSound_ptr,
        kInt64,
        kInt64_ptr,
        kBool,
        kBool_ptr
    };

    virtual ~dsp_memory_manager() {}
    virtual void begin(size_t) {}
    virtual void info(const char*, MemType, size_t, size_t, size_t, size_t) {}
    virtual void end() {}
    // Legacy overloads from the upstream architecture/faust/dsp/dsp.h.
    virtual void* allocate(size_t size) = 0;
    virtual void destroy(void* ptr) = 0;
    // Alignment-aware faust-rs mem0 extension: default implementations
    // forward to the legacy overloads above, so a manager that only
    // overrides those keeps working unchanged. Generated code prefers these
    // overloads when a manager provides them directly.
    virtual void* allocate(size_t size, size_t /*alignment*/) { return allocate(size); }
    virtual void destroy(void* ptr, size_t /*size*/, size_t /*alignment*/) { destroy(ptr); }
};

class AuditCppMemoryManager : public dsp_memory_manager {
    struct Description {
        std::string name;
        size_t sizeBytes;
        bool object;
        bool used;
    };

    size_t fAnnounced = 0;
    size_t fNext = 0;
    std::vector<Description> fDescriptions;
    std::vector<void*> fStack;
    std::map<void*, size_t> fLive;
    std::string fFailure;

    void fail(const std::string& message)
    {
        if (fFailure.empty()) fFailure = message;
    }

   public:
    void begin(size_t count) override
    {
        fAnnounced = count;
        fNext = 0;
        fDescriptions.clear();
    }

    void info(const char* name, MemType type, size_t, size_t sizeBytes, size_t, size_t) override
    {
        fDescriptions.push_back(
            {name ? name : "<null>", sizeBytes, type == kObj_ptr, false});
    }

    void end() override
    {
        if (fDescriptions.size() != fAnnounced) fail("description count mismatch");
    }

    void* allocate(size_t size) override
    {
        Description* match = nullptr;
        for (Description& zone : fDescriptions) {
            if (!zone.used && zone.sizeBytes == size) {
                match = &zone;
                break;
            }
        }
        if (!match) {
            for (Description& zone : fDescriptions) {
                if (!zone.used && zone.object) {
                    match = &zone;
                    break;
                }
            }
        }
        if (!match) {
            std::string unused;
            for (const Description& zone : fDescriptions) {
                if (!zone.used) {
                    if (!unused.empty()) unused += ", ";
                    unused += zone.name + "=" + std::to_string(zone.sizeBytes);
                }
            }
            fail("allocation " + std::to_string(fNext + 1) + " of "
                 + std::to_string(fDescriptions.size()) + " has no description for "
                 + std::to_string(size) + " bytes; unused: " + unused);
        } else {
            match->used = true;
        }
        ++fNext;
        void* ptr = std::malloc(size ? size : 1);
        if (ptr) {
            std::memset(ptr, 0xa5, size ? size : 1);
            fStack.push_back(ptr);
            fLive[ptr] = size;
        }
        return ptr;
    }

    void destroy(void* ptr) override
    {
        std::vector<void*>::iterator found =
            std::find(fStack.begin(), fStack.end(), ptr);
        if (found == fStack.end()) fail("destroyed pointer was not live");
        else fStack.erase(found);
        if (fLive.erase(ptr) != 1) fail("unknown or double-freed pointer");
        std::free(ptr);
    }

    // Alignment-aware faust-rs mem0 extension. Reuses the legacy matching and
    // bookkeeping logic above; additionally verifies the returned address
    // satisfies the requested alignment, and (on destroy) that the size
    // handed back matches what was recorded at allocation -- proof that
    // generated code is actually reaching this richer overload rather than
    // the legacy one.
    void* allocate(size_t size, size_t alignment) override
    {
        void* ptr = allocate(size);
        if (ptr != nullptr && alignment != 0
            && reinterpret_cast<std::uintptr_t>(ptr) % alignment != 0) {
            fail("allocate(size, alignment) returned an address that does not satisfy alignment "
                 + std::to_string(alignment));
        }
        return ptr;
    }

    void destroy(void* ptr, size_t size, size_t /*alignment*/) override
    {
        std::map<void*, size_t>::const_iterator recorded = fLive.find(ptr);
        if (recorded != fLive.end() && recorded->second != size) {
            fail("destroy(ptr, size, alignment) size " + std::to_string(size)
                 + " does not match the " + std::to_string(recorded->second)
                 + " bytes recorded at allocation");
        }
        destroy(ptr);
    }

    void verify() const
    {
        if (!fFailure.empty() || fDescriptions.size() != fAnnounced || fNext != fDescriptions.size()
            || !fLive.empty() || !fStack.empty()) {
            std::fprintf(stderr, "mem0 C++ audit failed: %s\n",
                         fFailure.empty() ? "incomplete lifecycle" : fFailure.c_str());
            std::abort();
        }
    }
};

#endif
