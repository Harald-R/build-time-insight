#pragma once

#include <cmath>

// Included by src/edge_unused.cpp, which references nothing from this header,
// so the header-usage analysis should classify it as "Unused".
namespace geom {

struct UnusedThing {
  int value = 0;

  // Inline members give the header enough parse work that clang's
  // -ftime-trace reliably records a Source/parse span for it.
  int doubled() const { return value * 2; }
  int tripled() const { return value * 3; }
  double hypot(double other) const {
    return std::sqrt(value * value + other * other);
  }
  int shifted(int by) const { return value << by; }
};

// Plain declaration: gives the analyzer's header-parse accounting a
// ParseDeclarationOrFunctionDefinition event for this header, which is what
// makes the header eligible for --header-usage classification.
int unused_thing_count();

} // namespace geom