# 2.0.0
* Unrecognized type letters will be treated as a primitive cell (fix #2).
* Breaking change in array support to make it compatible with existing solutions (fix #1). Example:
```C
public OnGameModeInit()
{
    new meaningOfLife[2];
    meaningOfLife[0] = 42;
    meaningOfLife[1] = 737;

    // Arrays are always passed as a pair of two letters: aA or ai
    // 'a' contains the actual array.
    // 'A' or 'i' should contain the size of the array, as shown.
    SetPreciseTimer("MeaningOfLife", 1000, false, "daA", playerid, meaningOfLife, sizeof(meaningOfLife));
}

// Callbacks receive the size of the array
forward MeaningOfLife(playerid,array[],array_size)
{
    for(new i = 0; i < array_size; i++)
    {
        printf("array[%d]=%d",i,array[i]);
    }
}
```
# 1.1.0
* Support for arrays.

# 1.0.0
Initial stable release. Supports basic AMX cells and strings.