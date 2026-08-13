#ifndef FAUSTFLOAT
#define FAUSTFLOAT double
#endif

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <string>
#include <vector>

#include "faust_minimal_cglue.h"
#include "../../../crates/ffi-common/include/faust-memory-manager.h"

extern "C" {
typedef struct mydsp mydsp;
mydsp* createmydsp(faust_memory_manager* manager);
void destroymydsp(mydsp* dsp);
void memoryInfomydsp(faust_memory_manager* manager);
void classInitmydsp(faust_memory_manager* manager, int sample_rate);
void classDestroymydsp(faust_memory_manager* manager);
void initmydsp(mydsp* dsp, int sample_rate);
int getNumInputsmydsp(mydsp* dsp);
int getNumOutputsmydsp(mydsp* dsp);
void buildUserInterfacemydsp(mydsp* dsp, UIGlue* ui_interface);
void computemydsp(mydsp* dsp, int count, FAUSTFLOAT** inputs, FAUSTFLOAT** outputs);
}

class AuditCMemoryManager {
    struct Description {
        std::string name;
        size_t sizeBytes;
        size_t alignment;
        bool used;
    };
    struct Allocation {
        size_t sizeBytes;
        size_t alignment;
    };

    size_t fAnnounced = 0;
    size_t fNext = 0;
    std::vector<Description> fDescriptions;
    std::vector<void*> fStack;
    std::map<void*, Allocation> fLive;
    std::string fFailure;

    void fail(const std::string& message)
    {
        if (fFailure.empty()) fFailure = message;
    }

    static AuditCMemoryManager* self(void* context)
    {
        return static_cast<AuditCMemoryManager*>(context);
    }

    static void begin(void* context, size_t count)
    {
        AuditCMemoryManager* audit = self(context);
        audit->fAnnounced = count;
        audit->fNext = 0;
        audit->fDescriptions.clear();
    }

    static void info(void* context, const char* name, faust_memory_type, size_t, size_t sizeBytes,
                     size_t alignment, uint64_t, uint64_t)
    {
        self(context)->fDescriptions.push_back(
            {name ? name : "<null>", sizeBytes, alignment, false});
    }

    static void end(void* context)
    {
        AuditCMemoryManager* audit = self(context);
        if (audit->fDescriptions.size() != audit->fAnnounced) {
            audit->fail("description count mismatch");
        }
    }

    static void* allocate(void* context, size_t sizeBytes, size_t alignment)
    {
        AuditCMemoryManager* audit = self(context);
        Description* match = nullptr;
        for (Description& zone : audit->fDescriptions) {
            if (!zone.used && zone.sizeBytes == sizeBytes && zone.alignment == alignment) {
                match = &zone;
                break;
            }
        }
        if (!match) audit->fail("allocation layout has no matching description");
        else match->used = true;
        ++audit->fNext;
        const size_t effectiveAlignment = std::max(alignment, sizeof(void*));
        void* ptr = nullptr;
#if defined(_WIN32)
        ptr = _aligned_malloc(sizeBytes ? sizeBytes : 1, effectiveAlignment);
#else
        if (posix_memalign(&ptr, effectiveAlignment, sizeBytes ? sizeBytes : 1) != 0) ptr = nullptr;
#endif
        if (ptr) {
            std::memset(ptr, 0xa5, sizeBytes ? sizeBytes : 1);
            audit->fStack.push_back(ptr);
            audit->fLive[ptr] = {sizeBytes, alignment};
        }
        return ptr;
    }

    static void destroy(void* context, void* ptr, size_t sizeBytes, size_t alignment)
    {
        AuditCMemoryManager* audit = self(context);
        std::vector<void*>::iterator stackEntry =
            std::find(audit->fStack.begin(), audit->fStack.end(), ptr);
        if (stackEntry == audit->fStack.end()) audit->fail("destroyed pointer was not live");
        else audit->fStack.erase(stackEntry);
        std::map<void*, Allocation>::iterator found = audit->fLive.find(ptr);
        if (found == audit->fLive.end()) {
            audit->fail("unknown or double-freed pointer");
        } else {
            if (found->second.sizeBytes != sizeBytes || found->second.alignment != alignment) {
                audit->fail("destroy layout differs from allocate");
            }
            audit->fLive.erase(found);
        }
#if defined(_WIN32)
        _aligned_free(ptr);
#else
        std::free(ptr);
#endif
    }

   public:
    faust_memory_manager api;

    AuditCMemoryManager()
    {
        api.abi_version = FAUST_MEMORY_MANAGER_ABI_VERSION;
        api.struct_size = sizeof(api);
        api.context = this;
        api.begin = begin;
        api.info = info;
        api.end = end;
        api.allocate = allocate;
        api.destroy = destroy;
    }

    void verify() const
    {
        if (!fFailure.empty() || fDescriptions.size() != fAnnounced || fNext != fDescriptions.size()
            || !fLive.empty() || !fStack.empty()) {
            std::fprintf(stderr, "mem0 C audit failed: %s\n",
                         fFailure.empty() ? "incomplete lifecycle" : fFailure.c_str());
            std::abort();
        }
    }
};

class Mem0CDsp {
    AuditCMemoryManager fManager;
    mydsp* fDSP;

   public:
    Mem0CDsp() : fDSP(nullptr)
    {
        memoryInfomydsp(&fManager.api);
        classInitmydsp(&fManager.api, 44100);
        fDSP = createmydsp(&fManager.api);
        if (!fDSP) std::abort();
    }

    ~Mem0CDsp()
    {
        destroymydsp(fDSP);
        classDestroymydsp(&fManager.api);
        fManager.verify();
    }

    void init(int sampleRate) { initmydsp(fDSP, sampleRate); }
    int getNumInputs() { return getNumInputsmydsp(fDSP); }
    int getNumOutputs() { return getNumOutputsmydsp(fDSP); }
    void buildUserInterface(UI* ui)
    {
        UIGlue glue;
        buildUIGlue(&glue, ui);
        buildUserInterfacemydsp(fDSP, &glue);
    }
    void compute(int count, FAUSTFLOAT** inputs, FAUSTFLOAT** outputs)
    {
        computemydsp(fDSP, count, inputs, outputs);
    }
};

#define FAUSTCLASS Mem0CDsp
#include "impulseexecopts_driver.h"
