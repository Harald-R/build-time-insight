#pragma once

#include "diamond_a.h" // -> diamond_a.h (a second, indirect path into it)

namespace geom {

struct DiamondB {
  int v = 2;
};

// Plain declaration so this header is attributed parse time (see unused.h).
int diamond_b_value();

} // namespace geom