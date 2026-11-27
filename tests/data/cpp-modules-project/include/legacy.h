#pragma once

#include <string>

// Plain header used alongside imports in the same translation unit.
struct Legacy {
    std::string name;
    int value;
};

inline int legacy_value(const Legacy& legacy) { return legacy.value; }
