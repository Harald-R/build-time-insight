module;
#include <cmath>
export module core:shapes;

namespace core {

export struct Point {
    double x;
    double y;
};

export inline double distance(Point a, Point b) {
    const double dx = a.x - b.x;
    const double dy = a.y - b.y;
    return std::sqrt(dx * dx + dy * dy);
}

} // namespace core
