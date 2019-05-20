# 1.1.0
* Support for arrays. Example:
```C
public OnGameModeInit()
{
    new meaningOfLife[2];
    meaningOfLife[0] = 42;
    meaningOfLife[1] = 737;

    // Arrays are always passed as a pair of two letters: Aa.
    // 'A' should contain the size of the array, as shown.
    // 'a' contains the actual array.
    SetPreciseTimer("MeaningOfLife", 1000, false, "dAa", playerid, sizeof(meaningOfLife), meaningOfLife);
}

// Callbacks receive the size of the array
forward MeaningOfLife(playerid,array_size,array[])
{
    for(new i = 0; i < array_size; i++)
    {
        printf("array[%d]=%d",i,array[i]);
    }
}
```

# 1.0.0
Initial stable release. Supports basic AMX cells and strings.