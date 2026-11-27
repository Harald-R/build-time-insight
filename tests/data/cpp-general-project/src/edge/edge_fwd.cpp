#include "fwd_class.h" // probe header: Widget used only through a pointer
#include "geometry.h"  // -> vector3.h

#include <vector>

// Uses Widget only through a pointer and references make_widget(), a function
// declared in fwd_class.h, but never constructs Widget. No definition is
// required, so the header-usage analysis must classify fwd_class.h as
// "DeclarationOnly" for this unit.
namespace geom {

double edge_fwd_work(const std::vector<Vector3<double>> &pts) {
  Widget *w = make_widget();
  (void)w;
  return centroid(pts).x();
}

} // namespace geom

double edge_fwd_entry() { return geom::edge_fwd_work({{1, 0, 0}, {2, 0, 0}}); }