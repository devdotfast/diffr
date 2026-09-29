program scanner
  implicit none
  integer :: i, j
  real :: total
  character(len=32) :: message
  total = 1.25e+2 + &
      & 2.5e-1
  message = 'hello &
      &world'
  do 100 i = 1, 3
    do 100 j = 1, 2
      total = total + real(i * j)
100 continue
  print *, message, total
end program scanner
