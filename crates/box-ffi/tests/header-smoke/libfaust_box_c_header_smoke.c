#include "../../include/libfaust-box-c.h"

int main(void)
{
    createLibContext();
    Box one = CboxInt(1);
    Box two = CboxInt(2);
    Box add = CboxAddAux(one, two);
    char* txt = CprintBox(add, true, 1024);
    freeCMemory(txt);
    (void)getCCompleteBoxError();
    destroyLibContext();
    return 0;
}
