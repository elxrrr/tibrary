"""Validated user-facing streaming and download settings."""

DEFAULTS = dict(
    request_interval_ms=750,
    request_timeout_sec=20,
    token_refresh_margin_sec=30,
    sign_in_timeout_sec=180,
    request_attempts=2,
    download_concurrency=1,
    segment_concurrency=1,
    download_delay=True,
    download_delay_min_sec=3.0,
    download_delay_max_sec=5.0,
    api_batch_size=20,
    api_batch_delay_sec=3.0,
    aac_bitrate_cap=320,
)


def normalized(values=None):
    data=dict(DEFAULTS,**(values or {}))
    def integer(key,low,high):data[key]=max(low,min(high,int(data[key])))
    def number(key,low,high):data[key]=max(low,min(high,float(data[key])))
    integer('request_interval_ms',100,5000)
    integer('request_timeout_sec',5,60)
    integer('token_refresh_margin_sec',10,300)
    integer('sign_in_timeout_sec',60,600)
    integer('request_attempts',1,4)
    integer('download_concurrency',1,4)
    integer('segment_concurrency',1,20)
    integer('api_batch_size',1,100)
    integer('aac_bitrate_cap',96,320)
    number('download_delay_min_sec',0,30)
    number('download_delay_max_sec',0,30)
    number('api_batch_delay_sec',0,60)
    if data['download_delay_max_sec']<data['download_delay_min_sec']:
        data['download_delay_max_sec']=data['download_delay_min_sec']
    data['download_delay']=bool(data['download_delay'])
    return data
