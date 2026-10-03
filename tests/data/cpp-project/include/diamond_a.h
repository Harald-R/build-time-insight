#pragma once

#include <cstddef>

// Diamond include pair: diamond_b.h includes diamond_a.h, and the edge-case
// units include the two in different orders, so diamond_a.h is reached via a
// different inclusion path in each unit (direct vs. via diamond_b.h).
namespace geom {

struct DiamondA {
  int v = 1;
};

// Plain declaration so this header is attributed parse time (see unused.h).
int diamond_a_value();

} // namespace geom