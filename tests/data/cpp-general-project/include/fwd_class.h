#pragma once

// A class used by two edge-case translation units:
// - src/edge_fwd.cpp uses Widget only through a pointer -> "DeclarationOnly"
// - src/edge_needs.cpp constructs a Widget -> "NeedsDefinition"
namespace geom {

class Widget {
public:
  int size() const { return 42; }
};

// Plain declaration so this header is attributed parse time (see unused.h).
Widget *make_widget();

} // namespace geom