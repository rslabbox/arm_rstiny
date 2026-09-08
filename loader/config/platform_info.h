/* Fixed QEMU virt contract, same structure as seL4's platform_sift.py output. */
int num_memory_regions = 1;
struct memory_region { size_t start; size_t end; } memory_region[1] = {
    { .start = 0x40000000, .end = 0x48000000 },
};
