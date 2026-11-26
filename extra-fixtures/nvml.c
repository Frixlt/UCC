#include <stdio.h>
#include <stdatomic.h>
#include <unistd.h>
#include <string.h>
#include <stdlib.h>
static _Atomic unsigned init_refs;
static _Atomic unsigned sleeping_probes;
unsigned nvmlInit_v2(void) {
    ++init_refs;
    if (getenv("UCC_E1_FIXTURE_SLOW_INIT")) {
        for (int i = 0; i < 1000 && init_refs < 2; ++i) usleep(1000);
    }
    return 0;
}
unsigned nvmlShutdown(void) { --init_refs; return 0; }
unsigned nvmlAuditRefCount(void) { return init_refs; }
unsigned nvmlAuditSleepingProbes(void) { return sleeping_probes; }
unsigned nvmlDeviceGetCount_v2(unsigned *out) {
    static _Atomic unsigned calls;
    if (++calls == 1 && getenv("UCC_AUDIT_FAIL_FIRST_NVML_COUNT")) return 999;
    const char *from = getenv("UCC_E16_FIXTURE_HOTPLUG_FROM");
    const char *to = getenv("UCC_E16_FIXTURE_HOTPLUG_TO");
    if (from && to) rename(from, to);
    *out = 2;
    return 0;
}
unsigned nvmlDeviceGetHandleByIndex_v2(unsigned index, void **out) {
    static int device;
    if (index >= 2) return 1;
    /* In reversed enumeration mode, NVML index 0 maps to the suspended BDF 03:00.0. */
    if ((getenv("UCC_E1_FIXTURE_REVERSED_ORDER") || getenv("UCC_AUDIT_REVERSED_NVML_INDEX")) && index == 0) ++sleeping_probes;
    if (getenv("UCC_E16_FIXTURE_FORWARD_ORDER") && index == 1) ++sleeping_probes;
    *out = &device;
    return 0;
}
unsigned nvmlDeviceGetName(void *dev, char *out, unsigned len) {
    if (len > sizeof("Fake GPU")) strcpy(out, "Fake GPU");
    return 0;
}
unsigned nvmlDeviceGetTemperature(void *dev, unsigned sensor, unsigned *out) { *out = 42; return 0; }
unsigned nvmlDeviceGetPowerUsage(void *dev, unsigned *out) { *out = 10000; return 0; }
unsigned nvmlDeviceGetEnforcedPowerLimit(void *dev, unsigned *out) { *out = 50000; return 0; }
unsigned nvmlDeviceGetPowerManagementLimitConstraints(void *dev, unsigned *min, unsigned *max) { *min = 10000; *max = 50000; return 0; }
unsigned nvmlDeviceSetPowerManagementLimit(void *dev, unsigned limit) { return 0; }
unsigned nvmlDeviceGetUtilizationRates(void *dev, unsigned *out) { out[0] = 0; out[1] = 0; return 0; }
unsigned nvmlDeviceGetMemoryInfo(void *dev, unsigned long long *out) { out[0] = 1024; out[1] = 1024; out[2] = 0; return 0; }

#ifdef AUDIT_WITH_BDF
static _Atomic unsigned bdf_calls;
unsigned nvmlAuditBdfCalls(void) { return bdf_calls; }
unsigned nvmlDeviceGetHandleByPciBusId_v2(const char *id, void **out) {
    ++bdf_calls;
    if (getenv("UCC_E16_FIXTURE_REFUSE_BDF")) return 4;
    if (getenv("UCC_E16_FIXTURE_HOTPLUG_TO") && !strcmp(id, "0000:02:00.0")) ++sleeping_probes;
    static int device;
    *out = &device;
    return 0;
}
#endif
