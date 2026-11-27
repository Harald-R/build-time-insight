import core;

#include "util.h"

#include <numeric>

int consume(int value) {
    const core::Ints values = core::build(value);
    return std::accumulate(values.begin(), values.end(), 0) +
           core::use_detail(value) + util::clamp(value, 0, 3);
}
